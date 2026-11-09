//! nasne の本体情報（読み取りのみ）

use crate::client::{Nasne, PORT_BOX};
use crate::json::{self, flag};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Info {
    pub name: String,
    pub model: String,
    pub software: String,
    /// nasne の時計（"2026-11-10 20:00:00"）
    pub clock: String,
    /// 放送波で時刻補正（TOT）を受信済みか
    pub tot: bool,
    pub recording: bool,
    /// いま配信しているクライアントの数
    pub streaming: i64,
    pub network: Network,
    pub hdd: Vec<Hdd>,
    pub settings: Settings,
    pub nas: Nas,
    /// DLNA クライアント（最終アクセスの新しい順）
    pub clients: Vec<Client>,
    /// nasne HOME（本体の設定画面）
    pub home_url: String,
    /// 取得できなかった項目
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Network {
    pub ip: String,
    pub dhcp: bool,
    pub subnet: String,
    pub gateway: String,
    pub dns: Vec<String>,
    pub mac: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Hdd {
    pub internal: bool,
    pub mounted: bool,
    pub product: String,
    pub format: String,
    /// バイト
    pub used: f64,
    pub total: f64,
    pub free: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Settings {
    pub power_saving: bool,
    pub bs_antenna_power: bool,
    pub event_relay: bool,
    pub frequency_follow: bool,
    /// 視聴年齢制限（0 = 制限なし）
    pub parental_rating: i64,
    /// DLNA クライアントを手動で登録する設定か
    pub dlna_manual_register: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Nas {
    pub enabled: bool,
    pub name: String,
    pub workgroup: String,
    pub shared: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Client {
    pub name: String,
    pub mac: String,
    pub registered: bool,
    /// "2026-11-10 20:00"
    pub last_access: String,
    pub user_agent: String,
}

const EPS: &[(&str, &str)] = &[
    ("box", "/status/boxNameGet"),
    ("hardware", "/status/hardwareVersionGet"),
    ("software", "/status/softwareVersionGet"),
    ("network", "/status/networkIfInfoGet"),
    ("time", "/status/currDateGet"),
    ("tot", "/status/TOTStatusGet"),
    ("powerSaving", "/status/HDDPowerSavingModeGet"),
    ("bsPower", "/status/bsPowerSupplyGet"),
    ("eventRelay", "/status/eventRelayInfoGet"),
    ("freqFollow", "/status/downloadingPermissionGet"),
    ("dmpAuto", "/status/DMPAutoRegisterInfoGet"),
    ("dmp", "/status/DMPListGet"),
    ("nas", "/status/NASInfoGet"),
    ("parental", "/status/parentalRatingInfoGet"),
    ("streaming", "/status/dtcpipClientListGet"),
    ("boxStatus", "/status/boxStatusListGet"),
];

fn datetime(s: &str, len: usize) -> String {
    s.replace('T', " ").chars().take(len).collect()
}

/// 状態系の API をまとめて並列に取得する（失敗した項目は errors に入れて続ける）
pub fn fetch(n: &Nasne) -> Info {
    let safe = |p: &str, q: &[(&str, String)]| n.get(PORT_BOX, p, q).unwrap_or_else(|e| json!({ "error": e }));
    let d: Value = std::thread::scope(|s| {
        let hdd = s.spawn(|| {
            let list = json::items(&safe("/status/HDDListGet", &[]), "HDD");
            let hs: Vec<_> = list
                .iter()
                .map(|h| {
                    let (id, h) = (json::i(h, "id").to_string(), h.clone());
                    s.spawn(move || {
                        let v = safe("/status/HDDInfoGet", &[("id", id)]);
                        v.get("HDD").cloned().unwrap_or(h)
                    })
                })
                .collect();
            Value::Array(hs.into_iter().filter_map(|h| h.join().ok()).collect())
        });
        let hs: Vec<_> = EPS.iter().map(|&(k, p)| (k, s.spawn(move || safe(p, &[])))).collect();
        let mut m = serde_json::Map::new();
        for (k, h) in hs {
            m.insert(k.into(), h.join().unwrap_or(Value::Null));
        }
        m.insert("hdd".into(), hdd.join().unwrap_or(Value::Null));
        Value::Object(m)
    });
    let mut info = from_json(&d);
    info.home_url = format!("http://{}:{PORT_BOX}/nasne_home/index.html", n.host);
    info
}

fn from_json(d: &Value) -> Info {
    let s = |v: &Value, k: &str| json::s(v, k).to_string();
    let (net, nas) = (&d["network"], &d["nas"]);
    let mut clients: Vec<Client> = json::items(&d["dmp"], "dmp")
        .iter()
        .map(|c| Client {
            name: s(c, "name"),
            mac: s(c, "macAddr"),
            registered: flag(c, "registerFlag"),
            last_access: datetime(json::s(c, "lastAccessDate"), 16),
            user_agent: s(c, "userAgent"),
        })
        .collect();
    clients.sort_by(|a, b| b.last_access.cmp(&a.last_access));
    let errors = EPS
        .iter()
        .filter_map(|&(k, p)| d[k].get("error").and_then(Value::as_str).map(|e| format!("{p}: {e}")))
        .collect();
    Info {
        name: s(&d["box"], "name"),
        model: s(&d["hardware"], "productName"),
        software: s(&d["software"], "softwareVersion"),
        clock: datetime(json::s(&d["time"], "currDate"), 19),
        tot: json::i(&d["tot"], "status") == 1,
        recording: !json::s(&d["boxStatus"]["tvTimerInfoStatus"], "nowId").is_empty(),
        streaming: json::i(&d["streaming"], "number"),
        network: Network {
            ip: s(net, "ipv4Addr"),
            dhcp: flag(net, "enableDHCPFlag"),
            subnet: s(net, "subnetMask"),
            gateway: s(net, "gateway"),
            dns: [s(net, "primaryDNS"), s(net, "secondaryDNS")].into_iter().filter(|x| !x.is_empty()).collect(),
            mac: s(net, "macAddr"),
        },
        hdd: d["hdd"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|h| Hdd {
                        internal: flag(h, "internalFlag"),
                        mounted: flag(h, "mountStatus"),
                        product: s(h, "productID"),
                        format: s(h, "format"),
                        used: json::f(h, "usedVolumeSize"),
                        total: json::f(h, "totalVolumeSize"),
                        free: json::f(h, "freeVolumeSize"),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        settings: Settings {
            power_saving: flag(&d["powerSaving"], "mode"),
            bs_antenna_power: flag(&d["bsPower"], "enable"),
            event_relay: flag(&d["eventRelay"], "eventRelayInfo"),
            frequency_follow: flag(&d["freqFollow"], "permission"),
            parental_rating: json::i(&d["parental"], "parentalRatingInfo"),
            dlna_manual_register: flag(&d["dmpAuto"], "autoRegisterInfo"),
        },
        nas: Nas {
            enabled: flag(nas, "enable"),
            name: s(nas, "nasName"),
            workgroup: s(nas, "workgroup"),
            shared: json::items(nas, "shared").iter().map(|x| s(x, "sharedName")).collect(),
        },
        clients,
        home_url: String::new(),
        errors,
    }
}

impl Nas {
    /// ファイルサーバーの接続先（smb://<IP>/<最初の共有フォルダ>）
    pub fn url(&self, ip: &str) -> String {
        format!("smb://{ip}/{}", self.shared.first().map(String::as_str).unwrap_or(""))
    }
}
