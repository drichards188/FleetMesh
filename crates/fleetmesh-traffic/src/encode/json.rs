//! JSON lines for what has no common wire format (docs/design/traffic.md section 5): Remote ID
//! as a receiver decodes it (the ASTM F3411 fields) and detections by the company's own sensors.
//! Serialized from structs, so the key order is fixed whatever serde_json features a binary
//! that links this crate enables.

use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RidMsg {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub rx: String,
    pub t_rx: i64,
    pub t: i64,
    pub uas_id: String,
    pub id_type: &'static str,
    pub ua_type: &'static str,
    pub status: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub alt_geo_m: f64,
    pub alt_baro_m: f64,
    pub height_agl_m: f64,
    pub speed_mps: f64,
    pub track_deg: f64,
    pub vspeed_mps: f64,
    pub h_acc_m: f64,
    pub v_acc_m: f64,
    pub op_lat: Option<f64>,
    pub op_lon: Option<f64>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct DetMsg {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub sensor: &'static str,
    pub platform: String,
    pub t: i64,
    pub sensor_lat: f64,
    pub sensor_lon: f64,
    pub sensor_alt_m: f64,
    pub bearing_deg: f64,
    pub sigma_bearing_deg: f64,
    pub elev_deg: Option<f64>,
    pub sigma_elev_deg: Option<f64>,
    pub range_m: Option<f64>,
    pub sigma_range_m: Option<f64>,
    pub radial_speed_mps: Option<f64>,
    pub size: &'static str,
}

/// Round to `d` decimals (so the line carries what the measurement resolves, no more).
pub fn round(v: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    let r = (v * m).round() / m;
    if r == 0.0 { 0.0 } else { r }
}

pub fn line<T: Serialize>(msg: &T) -> String {
    let mut s = serde_json::to_string(msg).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_id_line_has_the_contract_keys_in_order() {
        let m = RidMsg {
            kind: "rid",
            rx: "hub-pier".into(),
            t_rx: 1781456400123,
            t: 1781456400050,
            uas_id: "1581F5FKD229400B5E1J".into(),
            id_type: "serial",
            ua_type: "multirotor",
            status: "airborne",
            lat: 37.8075,
            lon: -122.4312,
            alt_geo_m: 96.0,
            alt_baro_m: 94.5,
            height_agl_m: 90.0,
            speed_mps: 6.2,
            track_deg: 271.0,
            vspeed_mps: 0.0,
            h_acc_m: 3.0,
            v_acc_m: 4.0,
            op_lat: Some(37.8069),
            op_lon: Some(-122.4288),
        };
        assert_eq!(
            line(&m),
            concat!(
                r#"{"type":"rid","rx":"hub-pier","t_rx":1781456400123,"t":1781456400050,"uas_id":"1581F5FKD229400B5E1J","#,
                r#""id_type":"serial","ua_type":"multirotor","status":"airborne","lat":37.8075,"lon":-122.4312,"#,
                r#""alt_geo_m":96.0,"alt_baro_m":94.5,"height_agl_m":90.0,"speed_mps":6.2,"track_deg":271.0,"#,
                r#""vspeed_mps":0.0,"h_acc_m":3.0,"v_acc_m":4.0,"op_lat":37.8069,"op_lon":-122.4288}"#,
                "\n"
            )
        );
        let d = DetMsg {
            kind: "det",
            sensor: "daa",
            platform: "12".into(),
            t: 1781456400200,
            sensor_lat: 37.80,
            sensor_lon: -122.45,
            sensor_alt_m: 90.0,
            bearing_deg: 41.5,
            sigma_bearing_deg: 2.0,
            elev_deg: Some(3.1),
            sigma_elev_deg: Some(2.0),
            range_m: None,
            sigma_range_m: None,
            radial_speed_mps: None,
            size: "medium",
        };
        assert_eq!(
            line(&d),
            concat!(
                r#"{"type":"det","sensor":"daa","platform":"12","t":1781456400200,"sensor_lat":37.8,"sensor_lon":-122.45,"#,
                r#""sensor_alt_m":90.0,"bearing_deg":41.5,"sigma_bearing_deg":2.0,"elev_deg":3.1,"sigma_elev_deg":2.0,"#,
                r#""range_m":null,"sigma_range_m":null,"radial_speed_mps":null,"size":"medium"}"#,
                "\n"
            )
        );
    }
}
