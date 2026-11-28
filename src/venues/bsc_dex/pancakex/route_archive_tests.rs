//! Перечитываем прежнюю запись SQLite и проверяем конкурентное изменение во время восстановления.
use super::*;

#[tokio::test]
async fn archive_old_route_uses_the_original_row_and_rejects_a_changed_attempt() {
    let m = marker();
    let dir = directory("route-wire-cas", 8453);
    let p = parts(&dir, 8453, m.ticket.req.owner).await;
    let db = p.ctx.journal.db();
    let mut r = request(&p, &m);
    r.route = Some(relay());
    // Форму прежнего журнала собираем для исходного ордера из архива unsigned quotes.
    for (key, mode, note) in [
        ("pcs", RouteMode::Relay(m.ticket.offer.hash), market::LABEL),
        ("generic", RouteMode::Orderbook(m.ticket.offer.hash), "Hanji"),
    ] {
        let mut order = serde_json::to_value(&r).unwrap();
        order["route"]["mode"] = json!(mode);
        order["route"]["exec_note"] = json!(note);
        let body = json!({"operation":{"Order":order},"lookup":{"venue_ref":m.ticket.offer.hash},"result":null});
        let bytes = body.to_string();
        db.ledger().write(move |c| c.execute(
            "INSERT INTO core_state(kind,id,body,updated_at) VALUES('settlement_action',?1,?2,'old')",
            rusqlite::params![key, bytes],
        )).await.unwrap();
        let previous = intent::load(db, key).await.unwrap().unwrap();
        let intent::Request::Order(req) = &previous.operation else { panic!("ожидается ордер") };
        let expected = if key == "pcs" { RouteMode::PancakeX(Some(m.ticket.offer.hash)) } else { mode };
        let mut loaded = req.route.clone().unwrap();
        assert_eq!(loaded.mode, expected);
        loaded.exec_note = Some(if key == "pcs" { "Ryze" } else { market::LABEL }.into());
        assert_eq!(market::is_route(&loaded), key == "pcs");
        // Внешнее поле старого/нового писателя не входит в модель Entry; CAS должен увидеть его изменение.
        let mut changed = body;
        changed["extra"] = json!({"opaque":17});
        let replacement = changed.to_string();
        db.ledger().write(move |c| c.execute("UPDATE core_state SET body=?1 WHERE id=?2", rusqlite::params![replacement,key])).await.unwrap();
        assert!(intent::archive_attempt(db, key, &previous, "test").await.is_err());
        let fresh = intent::load(db, key).await.unwrap().unwrap();
        let mut altered = fresh.clone();
        altered.lookup.nonce = Some(99);
        assert!(intent::archive_attempt(db, key, &altered, "test").await.is_err());

        let outcome = || intent::Outcome::Order(Box::new(super::super::result::empty(r.clone(), m.ticket.offer.hash)));
        assert!(intent::confirm(db, key, &previous, outcome()).await.is_err());
        assert!(intent::confirm(db, key, &altered, outcome()).await.is_err());
        intent::confirm(db, key, &fresh, outcome()).await.unwrap();
        let confirmed = intent::load(db, key).await.unwrap().unwrap();
        intent::archive_attempt(db, key, &confirmed, "test").await.unwrap();
        assert!(intent::load(db, key).await.unwrap().is_none());
        let closed = format!("{key}/closed/{}", m.ticket.offer.hash);
        let archived = db.ledger().read(move |c| c.query_row(
            "SELECT body FROM core_state WHERE kind='settlement_action' AND id=?1", [closed], |row| row.get::<_, String>(0),
        )).await.unwrap();
        let actual: Value = serde_json::from_str(&archived).unwrap();
        assert_eq!(actual["attempt"]["extra"], changed["extra"]);
        assert_eq!(actual["attempt"]["operation"], changed["operation"]);
        assert!(actual["attempt"]["result"]["Order"].is_object());
    }
}
