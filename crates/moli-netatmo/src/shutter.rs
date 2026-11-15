//! Bubendorff roller shutters (`NBR`, sold with iDiamant) on the Netatmo
//! API: the same `homesdata` / `homestatus` / `setstate` calls as the
//! cameras, with the `read_bubendorff` / `write_bubendorff` scopes.
//!
//! The opening follows Moli's convention (`Semantic::Position`, 100 = open,
//! 0 = closed), which is also Netatmo's `current_position` / `target_position`.

use std::collections::HashMap;

use anyhow::bail;
use http::Method;
use moli_core::{Access, Device, DeviceId, Kind, PointSpec, Semantic, Unit, Value};
use moli_runtime::{CommandRequest, DriverCtx};
use serde_json::{Value as Json, json};

use super::{api_call, native_id};

/// Module types of the Netatmo shutters (Bubendorff).
const SHUTTER_TYPES: [&str; 1] = ["NBR"];
/// `target_position` values that are not a position: Netatmo stops the
/// shutter where it is with -1.
const STOP: i64 = -1;
const ORDERS: [&str; 3] = ["open", "stop", "close"];

#[derive(Clone, Debug)]
pub struct ShutterInfo {
    pub home_id: String,
    pub mac_id: String,
    pub name: String,
    pub room: Option<String>,
}

pub fn parse_shutters(body: &Json, filter_home: Option<&str>) -> Vec<ShutterInfo> {
    let mut out = Vec::new();
    for home in body["body"]["homes"].as_array().into_iter().flatten() {
        let Some(home_id) = home["id"].as_str() else {
            continue;
        };
        if filter_home.is_some_and(|want| want != home_id) {
            continue;
        }
        let rooms: HashMap<&str, &str> = home["rooms"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| Some((r["id"].as_str()?, r["name"].as_str()?)))
            .collect();
        for m in home["modules"].as_array().into_iter().flatten() {
            if !m["type"]
                .as_str()
                .is_some_and(|t| SHUTTER_TYPES.contains(&t))
            {
                continue;
            }
            let Some(mac_id) = m["id"].as_str() else {
                continue;
            };
            out.push(ShutterInfo {
                home_id: home_id.to_owned(),
                mac_id: mac_id.to_owned(),
                name: m["name"]
                    .as_str()
                    .map_or_else(|| "Bubendorff".to_owned(), |n| n.trim().to_owned()),
                room: m["room_id"]
                    .as_str()
                    .and_then(|rid| rooms.get(rid).copied())
                    .map(str::to_owned),
            });
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShutterStatus {
    pub online: bool,
    /// Where the shutter is, in %: 100 = open.
    pub position: Option<f64>,
    /// `up`, `down` or `stopped`, from where it is going.
    pub moving: Option<&'static str>,
}

pub fn decode_shutter_status(module: &Json) -> ShutterStatus {
    let position = module["current_position"].as_f64();
    let moving = match (position, module["target_position"].as_f64()) {
        (Some(now), Some(target)) if target > now => Some("up"),
        (Some(now), Some(target)) if target < now => Some("down"),
        (Some(_), Some(_)) => Some("stopped"),
        _ => None,
    };
    ShutterStatus {
        online: module["reachable"].as_bool().unwrap_or(true),
        position,
        moving,
    }
}

pub fn build_device(ctx: &DriverCtx, id: &DeviceId, shutter: &ShutterInfo) -> Device {
    let point = |key: &str, label: String, kind: Kind, write: bool, semantic: Semantic| PointSpec {
        key: key.into(),
        label: label.into(),
        kind,
        access: Access {
            read: key != "control",
            write,
        },
        unit: (key == "position").then_some(Unit::Percent),
        semantic,
    };
    let words = |values: &[&str]| Kind::Enum {
        values: values.iter().map(|v| (*v).into()).collect(),
    };
    Device {
        id: id.clone(),
        instance: ctx.instance().clone(),
        native_name: shutter.name.clone().into(),
        manufacturer: Some("Bubendorff".into()),
        model: Some("iDiamant".into()),
        description: Some(moli_i18n::tr!("pilotes.netatmo.volet").into()),
        native_room: shutter.room.as_deref().map(Into::into),
        members: Vec::new(),
        points: vec![
            point(
                "position",
                moli_i18n::tr!("pilotes.netatmo.ouverture"),
                Kind::Numeric {
                    min: Some(0.0),
                    max: Some(100.0),
                    step: Some(1.0),
                },
                true,
                Semantic::Position,
            ),
            // An order, never a switch: what the dashboard and the assistant
            // look for to open, stop and close a shutter.
            point(
                "control",
                moli_i18n::tr!("pilotes.netatmo.commande"),
                words(&ORDERS),
                true,
                Semantic::Other,
            ),
            point(
                "moving",
                moli_i18n::tr!("pilotes.netatmo.deplacement"),
                words(&["up", "down", "stopped"]),
                false,
                Semantic::Other,
            ),
        ],
    }
}

pub fn publish(ctx: &DriverCtx, id: &DeviceId, status: &ShutterStatus) {
    ctx.set_availability(id, status.online);
    if let Some(position) = status.position {
        ctx.set_state(id, "position", Value::Float(position));
    }
    if let Some(moving) = status.moving {
        ctx.set_state(id, "moving", Value::Text(moving.into()));
    }
}

/// The `target_position` an order of the `position` or `control` point asks.
fn target(key: &str, value: &Value) -> anyhow::Result<i64> {
    match (key, value) {
        ("position", v) => match v.as_f64() {
            #[allow(clippy::cast_possible_truncation)]
            Some(p) if (0.0..=100.0).contains(&p) => Ok(p.round() as i64),
            _ => bail!(moli_i18n::tr!("pilotes.netatmo.position_invalide")),
        },
        ("control", Value::Text(order)) => match &**order {
            "open" => Ok(100),
            "close" => Ok(0),
            "stop" => Ok(STOP),
            _ => bail!(moli_i18n::tr!(
                "pilotes.netatmo.point_inconnu",
                point = &**order
            )),
        },
        _ => bail!(moli_i18n::tr!("pilotes.netatmo.point_inconnu", point = key)),
    }
}

pub async fn handle_command(
    ctx: &mut DriverCtx,
    token: &str,
    shutter: &ShutterInfo,
    id: &DeviceId,
    cmd: CommandRequest,
) {
    let outcome: anyhow::Result<()> = async {
        let target = target(&cmd.key, &cmd.value)?;
        let body = json!({
            "home": {
                "id": shutter.home_id,
                "modules": [{ "id": shutter.mac_id, "target_position": target }]
            }
        });
        let (status, answer) = api_call(Method::POST, "/api/setstate", token, Some(&body)).await?;
        if !status.is_success() {
            bail!("HTTP {} {}", status.as_u16(), answer);
        }
        // The next poll tells where it really is; until then, where it goes.
        #[allow(clippy::cast_precision_loss)]
        if target != STOP {
            ctx.set_state(id, "position", Value::Float(target as f64));
        }
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => cmd.reply(Ok(())),
        Err(e) => cmd.reply(Err(moli_i18n::tr!(
            "pilotes.netatmo.echec",
            why = format!("{e:#}")
        ))),
    }
}

/// The shutters of a `homesdata` answer, by device id.
pub fn discover(
    ctx: &DriverCtx,
    body: &Json,
    filter_home: Option<&str>,
) -> HashMap<DeviceId, ShutterInfo> {
    parse_shutters(body, filter_home)
        .into_iter()
        .map(|s| (ctx.device_id(&native_id(&s.mac_id)), s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Json {
        serde_json::from_slice(include_bytes!(
            "../../../integrations/netatmo/fixtures/homestatus.json"
        ))
        .unwrap()
    }

    #[test]
    fn finds_the_shutter_and_leaves_the_cameras() {
        let found = parse_shutters(&fixture()["homesdata"], None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Volet");
        assert_eq!(found[0].room.as_deref(), Some("Salon"));
        assert!(parse_shutters(&fixture()["homesdata"], Some("other")).is_empty());
    }

    #[test]
    fn reads_position_and_direction() {
        let status = &fixture()["homestatus"]["body"]["home"]["modules"][2];
        let decoded = decode_shutter_status(status);
        assert!(decoded.online);
        assert_eq!(decoded.position, Some(40.0));
        assert_eq!(decoded.moving, Some("up"));

        let still = json!({"current_position": 100, "target_position": 100, "reachable": false});
        let decoded = decode_shutter_status(&still);
        assert!(!decoded.online);
        assert_eq!(decoded.moving, Some("stopped"));
        assert_eq!(decode_shutter_status(&json!({})).moving, None);
    }

    #[test]
    fn orders_become_target_positions() {
        let text = |t: &str| Value::Text(t.into());
        assert_eq!(target("control", &text("open")).unwrap(), 100);
        assert_eq!(target("control", &text("close")).unwrap(), 0);
        assert_eq!(target("control", &text("stop")).unwrap(), STOP);
        assert_eq!(target("position", &Value::Float(41.6)).unwrap(), 42);
        assert!(target("position", &Value::Float(101.0)).is_err());
        assert!(target("position", &text("40")).is_err());
        assert!(target("control", &text("tilt")).is_err());
        assert!(target("moving", &text("up")).is_err());
    }
}
