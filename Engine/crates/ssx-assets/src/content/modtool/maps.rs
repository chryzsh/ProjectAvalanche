use std::collections::BTreeSet;
use std::path::Path;

use ssx_content::check::{Checked, Severity};
use ssx_content::map::{MAX_BODIES, MAX_COLLISION_TRIANGLES, MAX_ENTITIES, MAX_VOLUMES, PropLook};
use ssx_content::object::{ObjectCollision, ObjectDef};
use ssx_content::{BASE_NAMESPACE, ContentId, PORT_NAMESPACE, Source};

use super::check::push;
use crate::content::markers::PortMarkers;
use crate::content::models::DiscModels;
use crate::content::objects::DiscRecords;

pub(super) fn check(c: &mut Checked, big: &Path, base: Option<&BTreeSet<ContentId>>) {
    let maps: Vec<_> = c.maps.iter().map(|(id, (f, m))| (id.clone(), f.clone(), m.clone())).collect();
    if maps.is_empty() {
        return;
    }
    let mut records: Option<Result<DiscRecords, String>> = None;
    for (id, file, map) in maps {
        let scene = map.toml.scene.clone();
        for (rid, what) in [(&map.toml.sky, "sky"), (&map.toml.postfx, "postfx"), (&map.toml.music, "music")] {
            if let Some(rid) = rid.as_ref().filter(|r| r.namespace() == PORT_NAMESPACE)
                && *rid != ssx_content::sky::scattering_id()
            {
                push(
                    c,
                    Severity::Error,
                    &file,
                    (0, 0),
                    format!("{what} {rid}: the port's own sky is ava:sky/scattering"),
                );
            }
        }
        for mk in map.definition.markers.iter().filter(|mk| mk.marker.namespace() == PORT_NAMESPACE) {
            if !PortMarkers.has(&mk.marker) {
                let message = format!(
                    "node \"{}\" ava_marker {}: the port's markers are ava:marker/{{ring, cylinder, arrow, chevron, beam, \
                     gate}}",
                    mk.node, mk.marker
                );
                push(c, Severity::Error, &scene, (0, 0), message);
            }
        }
        if let Some(b) = map.toml.base.as_ref().filter(|b| base.is_some_and(|ids| ids.contains(b))) {
            match crate::content::GameContent::of(big).layout_base(b) {
                Ok(l) => {
                    let message =
                        format!("{id}: a layout on {b}: {} drop {} (ge_drop 0x{:08x})", l.track, l.index, l.ge_drop);
                    push(c, Severity::Note, &file, (0, 0), message);
                }
                Err(e) => push(c, Severity::Error, &file, (0, 0), format!("base {b}: {e:#}")),
            }
        }
        let content = crate::content::GameContent::of(big);
        let d = &map.definition;
        let named = d.terrain.iter().filter_map(|t| t.material.as_ref().map(|m| (&t.node, m)));
        for (node, mid) in named.chain(d.rails.iter().filter_map(|r| r.material.as_ref().map(|m| (&r.node, m)))) {
            if base.is_some_and(|ids| ids.contains(mid))
                && let Err(e) = content.map_terrain_material(mid)
            {
                push(c, Severity::Error, &scene, (0, 0), format!("node \"{node}\" ava_material: {e:#}"));
            }
        }
        for t in d.terrain.iter().filter(|t| t.texture.is_some()) {
            let image = map.terrain_image(t).and_then(|k| map.props.as_ref()?.model.images.get(k).map(|i| (k, i)));
            let message = match image {
                Some((k, i)) => format!(
                    "node \"{}\": its base colour texture (image {k}, {}) is drawn instead of its terrain material's \
                     base surfaces; the snow stays the material's",
                    t.node, i.file_name
                ),
                None => {
                    format!("node \"{}\": its material's base colour texture is not among the scene's images", t.node)
                }
            };
            push(c, if image.is_some() { Severity::Note } else { Severity::Error }, &scene, (0, 0), message);
        }
        if let Some(t) = map.toml.midground.as_ref().filter(|t| base.is_some_and(|ids| ids.contains(t)))
            && let Err(e) = content.map_midground(t)
        {
            push(c, Severity::Error, &file, (0, 0), format!("midground {t}: {e:#}"));
        }
        let mut triangles = map.definition.collision_triangles();
        let mut entities = Entities::default();
        for p in &map.definition.props {
            let PropLook::Object(oid) = &p.look else { continue };
            let Some((_, def)) = c.objects.get(oid).filter(|_| oid.namespace() != BASE_NAMESPACE).cloned() else {
                continue;
            };
            if let Some(ctl) = def.controller.as_ref().filter(|ctl| ctl.namespace() != BASE_NAMESPACE) {
                let message = format!(
                    "node \"{}\" ava_object {oid}: its controller {ctl} is a package's script: a placed object's \
                     controller is one of the disc's (ssx:script/<path>); the scripting API has no entity controller",
                    p.node
                );
                push(c, Severity::Error, &scene, (0, 0), message);
            }
            entities.count(&def, 0);
        }
        let mut placed = BTreeSet::new();
        let mut disc_entities: Vec<(Option<u32>, bool, bool)> = Vec::new();
        for p in &map.definition.props {
            let PropLook::Object(oid) = &p.look else { continue };
            if oid.namespace() != BASE_NAMESPACE || !base.is_some_and(|ids| ids.contains(oid)) {
                continue;
            }
            let r = records.get_or_insert_with(|| {
                DiscModels::index(big)
                    .map(|m| DiscRecords::new(big, std::sync::Arc::new(m)))
                    .map_err(|e| format!("{e:#}"))
            });
            let def = match r {
                Ok(r) => r
                    .load(oid)
                    .map_err(|e| e.to_string())
                    .and_then(|a| a.into_value::<ObjectDef>().ok_or_else(|| "not an object record".to_string())),
                Err(e) => Err(format!("the disc's models cannot be indexed: {e}")),
            };
            let def = match def {
                Ok(d) => d,
                Err(e) => {
                    push(c, Severity::Error, &scene, (0, 0), format!("node \"{}\" ava_object {oid}: {e}", p.node));
                    continue;
                }
            };
            let key = r.as_ref().ok().and_then(|r| r.names().ok()).and_then(|n| n.by_id.get(oid).copied());
            disc_entities.push((key.map(|(_, k)| k), def.controller.is_some(), def.rigid_body.is_some()));
            if let ObjectCollision::Decoded { collision, .. } = &def.collision {
                triangles += collision.triangle_count();
            }
            placed.insert(oid.clone());
        }
        if triangles > MAX_COLLISION_TRIANGLES {
            let message = format!(
                "{id}: {triangles} collision triangles with the disc objects it places; at most \
                 {MAX_COLLISION_TRIANGLES}, the disc's largest track (a package object's collision counts too, at load)"
            );
            push(c, Severity::Error, &file, (0, 0), message);
        } else if !placed.is_empty() {
            let message = format!(
                "{id}: {triangles} collision triangles with the {} disc objects it places (at most {MAX_COLLISION_TRIANGLES})",
                placed.len()
            );
            push(c, Severity::Note, &file, (0, 0), message);
        }
        let keys: BTreeSet<u32> = disc_entities.iter().filter_map(|(k, ..)| *k).collect();
        let volumes: std::collections::BTreeMap<u32, usize> = match keys.is_empty() {
            true => Default::default(),
            false => crate::vault_db::with_attribdb(big, |db| {
                let count = |k: u32| {
                    let obj =
                        ssx_formats::attrib::RefSpec { class: ssx_rules::locators::CLASS_GAMEOBJECT, collection: k };
                    anyhow::Ok((k, ssx_rules::locators::object_volumes(db, obj)?.len()))
                };
                keys.iter().map(|&k| count(k)).collect()
            })
            .unwrap_or_else(|e| {
                push(c, Severity::Error, &scene, (0, 0), format!("{id}: the disc objects' trigger volumes: {e:#}"));
                Default::default()
            }),
        };
        for (key, controller, body) in disc_entities {
            entities.count_parts(controller, body, key.and_then(|k| volumes.get(&k)).copied().unwrap_or(0));
        }
        let map_volumes = map.definition.checkpoints.len()
            + map.definition.triggers.len()
            + map.definition.deaths.len()
            + usize::from(map.definition.finish.is_some());
        for message in entities.problems(map_volumes) {
            push(c, Severity::Error, &file, (0, 0), format!("{id}: {message}"));
        }
        if entities.entities > 0 {
            let message = format!(
                "{id}: {} entities (placed objects with trigger volumes or a controller: {} with a controller, {} with \
                 a rigid body it moves, {} trigger volumes)",
                entities.entities, entities.scripted, entities.bodies, entities.volumes
            );
            push(c, Severity::Note, &file, (0, 0), message);
        }
        if map.toml.base.is_none() && map.definition.deaths.is_empty() {
            let message = format!(
                "{id}: a new track without a death volume: a rider who leaves the ground falls until the ride ends \
                 (the game has no other floor: `content.md` §22.1 (c))"
            );
            push(c, Severity::Warning, &file, (0, 0), message);
        }
    }
}

#[derive(Default)]
struct Entities {
    entities: usize,
    scripted: usize,
    bodies: usize,
    volumes: usize,
}

impl Entities {
    fn count(&mut self, def: &ObjectDef, volumes: usize) {
        self.count_parts(def.controller.is_some(), def.rigid_body.is_some(), volumes);
    }

    fn count_parts(&mut self, controller: bool, body: bool, volumes: usize) {
        if volumes == 0 && !controller {
            return;
        }
        self.entities += 1;
        self.volumes += volumes;
        if controller {
            self.scripted += 1;
            self.bodies += usize::from(body);
        }
    }

    fn problems(&self, map_volumes: usize) -> Vec<String> {
        let mut out = Vec::new();
        if self.entities > MAX_ENTITIES {
            out.push(format!(
                "{} entities (placed objects with trigger volumes or a controller); at most {MAX_ENTITIES}",
                self.entities
            ));
        }
        if self.bodies > MAX_BODIES {
            out.push(format!(
                "{} placed objects with a rigid body and a controller; at most {MAX_BODIES}",
                self.bodies
            ));
        }
        if map_volumes + self.volumes > MAX_VOLUMES {
            out.push(format!(
                "{map_volumes} volumes of its own and {} of its objects; at most {MAX_VOLUMES} in all",
                self.volumes
            ));
        }
        out
    }
}
