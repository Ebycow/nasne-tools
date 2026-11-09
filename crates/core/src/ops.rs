//! nasne への操作（一覧の取得・予約・録画の変更など）。GUI もコマンドもここを通す。

use crate::client::{self, LIST_Q, Nasne, PORT_BOX, PORT_REC, Res};
use crate::model::{Failed, Recording, RecordingUpdate, ReserveReq, Reservation, Status};
use crate::{gaiji, info, json, jst};

fn q(v: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    v.iter().map(|&(k, v)| (k, v.to_string())).collect()
}

impl Nasne {
    /// 名前・HDD 残量・録画中か
    pub fn status(&self) -> Res<Status> {
        std::thread::scope(|s| {
            let a = s.spawn(|| self.get(PORT_BOX, "/status/boxNameGet", &[]));
            let b = s.spawn(|| self.get(PORT_BOX, "/status/HDDInfoGet", &[("id", "0".into())]));
            let c = self.get(PORT_BOX, "/status/boxStatusListGet", &[]);
            let failed = || Err("取得に失敗しました".to_string());
            let (a, b) = (a.join().unwrap_or_else(|_| failed())?, b.join().unwrap_or_else(|_| failed())?);
            let (c, h) = (c?, &b["HDD"]);
            Ok(Status {
                name: json::s(&a, "name").to_string(),
                used: json::f(h, "usedVolumeSize"),
                total: json::f(h, "totalVolumeSize"),
                free: json::f(h, "freeVolumeSize"),
                recording: !json::s(&c["tvTimerInfoStatus"], "nowId").is_empty(),
            })
        })
    }

    /// 本体情報（読み取りのみ）
    pub fn info(&self) -> info::Info {
        info::fetch(self)
    }

    /// 本体のランプを点灯させる（本体を見つけるため）
    pub fn blink(&self) -> Res<()> {
        let p = q(&[("powerGreen", "2"), ("powerRed", "1"), ("record", "1"), ("network", "1"), ("hddAlert", "1")]);
        self.get(PORT_BOX, "/config/LEDControl", &p).and_then(client::check).map(drop)
    }

    // ------------------------------------------------------------ 予約
    /// 予約一覧（開始時刻順）
    pub fn reservations(&self) -> Res<Vec<Reservation>> {
        let v = self.get_list(PORT_REC, "/schedule/reservedListGet")?;
        let mut r: Vec<Reservation> = json::items(&v, "item").iter().map(Reservation::from).collect();
        r.sort_by_key(|r| r.start);
        Ok(r)
    }

    /// その時間に重なっている予約（nasne のチューナーは 1 つ）
    pub fn conflicts(&self, r: &ReserveReq) -> Res<Vec<Reservation>> {
        let c = self.get(PORT_REC, "/schedule/conflictListGet", &time_params(r))?;
        if json::i(&c, "numberReturned") == 0 {
            return Ok(Vec::new());
        }
        Ok(json::items(&c, "item").iter().map(Reservation::from).collect())
    }

    /// 録画予約する（重なりは確認しない。先に conflicts で確かめる）
    pub fn reserve(&self, r: &ReserveReq) -> Res<()> {
        let mut p = vec![("title", r.title.clone())];
        p.extend(time_params(r));
        p.extend([
            ("conditionId", r.cond.clone()),
            ("quality", r.quality.to_string()),
            ("eventId", r.eid.to_string()),
            ("priorityFlag", "1".into()),
            ("storageId", "0".into()),
            ("creatorId", "2000".into()),
            ("forceFlag", "1".into()),
            ("parentalId", "0".into()),
        ]);
        self.post(PORT_REC, "/schedule/reservedInfoCreate", &p).map(drop)
    }

    /// 予約の画質・繰り返しを変える。raise_priority なら、重なった時間にこの予約を録画させる
    pub fn update_reservation(&self, id: &str, quality: i64, cond: &str, raise_priority: bool) -> Res<()> {
        let p = [
            ("id", id.to_string()),
            ("quality", quality.to_string()),
            ("conditionId", cond.to_string()),
            ("priorityChange", (raise_priority as i32).to_string()),
            ("forceFlag", "1".into()),
            ("storageId", "0".into()),
        ];
        self.post(PORT_REC, "/schedule/reservedInfoUpdate", &p).map(drop)
    }

    pub fn cancel_reservation(&self, id: &str) -> Res<()> {
        self.post(PORT_REC, "/schedule/reservedInfoDelete", &[("id", id.to_string()), ("forceFlag", "1".into())]).map(drop)
    }

    /// 録画できなかった予約
    pub fn failed(&self) -> Res<Vec<Failed>> {
        let v = self.get(PORT_BOX, "/status/recNgListGet", &[])?;
        let mut a = json::items(&v, "item");
        if a.is_empty() {
            a = json::items(&v, "recNgList");
        }
        Ok(a.iter().map(Failed::from).collect())
    }

    /// 録画できなかった予約を一覧から消す
    pub fn clear_failed(&self, id: &str) -> Res<()> {
        self.post(PORT_BOX, "/config/recNgListDelete", &[("id", id.to_string())]).map(drop)
    }

    // ------------------------------------------------------------ 録画
    pub fn recordings(&self) -> Res<Vec<Recording>> {
        let v = self.get_list(PORT_REC, "/recorded/titleListGet")?;
        Ok(json::items(&v, "item").iter().map(Recording::from).collect())
    }

    /// タイトル・保護・未視聴を変える
    pub fn update_recording(&self, id: &str, u: &RecordingUpdate) -> Res<()> {
        // titleUpdate は id / title / protectFlag / newFlag を毎回すべて送る必要がある。
        // 表示用に [字] などへ変換したタイトルを送り返すと nasne 上の外字が壊れるため、
        // 改名しないときは nasne から取り直した生のタイトルを使う。
        let list = self.get_raw(PORT_REC, "/recorded/titleListGet", &q(LIST_Q))?;
        let items = json::items(&list, "item");
        let cur = items.iter().find(|x| json::s(x, "id") == id).ok_or("録画が見つかりません")?;
        let title = match &u.title {
            Some(t) => gaiji::from_text(t),
            None => json::s(cur, "title").to_string(),
        };
        let flag = |o: Option<bool>, k: &str| o.map(|b| b as i64).unwrap_or(json::i(cur, k)).to_string();
        let p = [
            ("id", id.to_string()),
            ("title", title),
            ("protectFlag", flag(u.protect, "protectFlag")),
            ("newFlag", flag(u.unwatched, "newFlag")),
        ];
        self.post(PORT_REC, "/recorded/titleUpdate", &p).map(drop)
    }

    /// 録画を削除する（元に戻せない）
    pub fn delete_recording(&self, id: &str) -> Res<()> {
        self.post(PORT_REC, "/recorded/titleDelete", &[("id", id.to_string())]).map(drop)
    }
}

fn time_params(r: &ReserveReq) -> Vec<(&'static str, String)> {
    vec![
        ("startDateTime", jst::dt(r.start).iso()),
        ("duration", r.dur.to_string()),
        ("broadcastingType", r.band.code().to_string()),
        ("serviceId", r.sid.to_string()),
    ]
}
