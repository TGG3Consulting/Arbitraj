//! Файл: core/uncapped_view.rs
//! Что делает: данные экрана «Упущено из-за потолка» (решение 141) за период: два итога — «по исполненным сделкам»
//! (боевая плановая прибыль, фактический итог сделки из учёта, прибыль той же моделью на нашем бюджете, потенциал без
//! потолка, упущено, сколько упёрлось в бюджет) и «по не исполненным кандидатам» (эпизоды: упущено — пиковое в эпизоде,
//! сколько отклонённых стали бы плюсовыми без потолка); таблица «потолок → прибыль» (значения — uncapped.ladder_usdt,
//! «взяли бы» = прибыль лучшего объёма в потолке, если она проходит пороги, иначе 0) и строка «без потолка»; топ
//! направлений по упущенному; счётчики очереди (посчитано, слито, не посчитано, сбои, мс на расчёт). Список записей
//! периода страницей, одна запись, запись сделки по её id.
//! Решение 197: кандидаты с упущено 0 (старые записи) в итог не входят — число скрытых hidden_zero; у записи сделки в
//! списке — факт из учёта.
//! Решение 215: итоги периода (uncapped_totals) — общие у экрана и отчёта Excel «Упущено» (core/uncapped_report.rs).
//! Решение 217: итог deals_old — записи сделок, посчитанные после покупки (занижено), отдельно; у тел записей без поля
//! basis (старые) оно дописывается как «live_after_buy». Решение 239: в deals_old — все записи сделок не по данным
//! момента входа (не decision_inputs) — «несравнимо».
//! Решение 218: у сделки в итогах и списке — одна, последняя запись (uncapped_sum.rs latest_per_deal): пересчёт старой
//! записи по окну (basis recomputed_window*) — основной для сделки, старая остаётся в базе.
//! Решение 245: у записи кандидата — показ окна максимального объёма рядом с длительностью сделок пары и вывод «успели
//! бы» (`maxwin_view`, maxwin_view.rs); в итогах — почему окна не ведутся или медианы нет (maxwin_off, deal_time_off).
//! Кто вызывает: api/uncapped.rs, core/uncapped_report.rs.
//! От чего зависит: storage/uncapped_db.rs, uncapped_sum.rs, uncapped_types.rs, uncapped_state.rs, uncapped.rs (ladder), ledger (сделки).
//! Настройки: uncapped.enabled, uncapped.ladder_usdt (только показ и причина).

use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use crate::storage::uncapped_db::{self, Filter};

use super::engine::{Core, lk};
use super::uncapped_sum::{Totals, latest_per_deal, totals};
use super::uncapped_types::Brief;

impl Core {
    /// Что делает: итоги периода [from, to) (RFC 3339; None — без границы) — общая часть экрана и отчёта «Упущено»
    /// (решение 215): записи базы, таблица потолков uncapped.ladder_usdt (не задана — пустая и причина), факт сделок
    /// из учёта, uncapped_sum.rs totals. Возвращает: (итоги, причина без таблицы потолков). Ошибки: текст сбоя базы.
    pub async fn uncapped_totals(
        &self,
        from: Option<String>,
        to: Option<String>,
    ) -> Result<(Totals, Option<String>), String> {
        let f = Filter {
            from,
            to,
            kind: None,
            missed_only: false,
        };
        // Решение 218: у сделки — только последняя запись (пересчёт по окну заменяет старую).
        let rows = latest_per_deal(
            uncapped_db::all(&self.inner.deps.db, f)
                .await
                .map_err(|e| e.to_string())?,
        );
        let (ladder, ladder_err) = match super::uncapped::ladder(self) {
            Ok(l) => (l, None),
            Err(e) => (vec![], Some(e)),
        };
        let briefs: Vec<Brief> = rows
            .iter()
            .filter_map(|(_, body)| serde_json::from_str::<Brief>(body).ok())
            .collect();
        let facts = lk(&self.inner.deals);
        let t = totals(briefs, &ladder, |id| {
            facts.get(id).and_then(|d| d.result_usd)
        });
        drop(facts);
        Ok((t, ladder_err))
    }

    /// Что делает: итоги за период [from, to) (RFC 3339; None — без границы), топ `top` направлений.
    /// Возвращает: {off, deals, candidates, ladder, top, stats}. Ошибки: текст сбоя базы.
    pub async fn uncapped_summary(
        &self,
        from: Option<String>,
        to: Option<String>,
        top: usize,
    ) -> Result<Value, String> {
        let (t, ladder_err) = self.uncapped_totals(from, to).await?;
        let ladder_rows: Vec<Value> = t
            .ladder
            .iter()
            .map(|(c, s)| json!({"cap": c, "deals": s[0], "candidates": s[1]}))
            .collect();
        let top_v: Vec<Value> = t
            .top
            .iter()
            .take(top)
            .map(|x| {
                let d = &x.dir;
                let key = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
                json!({"direction": key, "missed": x.missed, "count": x.count})
            })
            .collect();
        let mw = self.maxwin_ctx();
        Ok(json!({
            "off": self.uncapped_off(),
            "maxwin_off": mw.gap.err(),
            "deal_time_off": mw.min_n.err(),
            "hidden_zero": t.hidden,
            "deals": t.deals.json(),
            "deals_old": t.deals_old.json(),
            "candidates": t.cands.json(),
            "ladder": ladder_rows,
            "ladder_error": ladder_err,
            "uncapped": {"deals": t.deals.potential, "candidates": t.cands.potential},
            "top": top_v,
            "stats": self.uncapped_stats(),
        }))
    }

    /// Страница записей периода, новые сверху: {rows: [{id, body}], total}. Ошибки: текст сбоя базы.
    pub async fn uncapped_list(
        &self,
        f: Filter,
        limit: usize,
        offset: usize,
    ) -> Result<Value, String> {
        let rows = latest_per_deal(
            uncapped_db::all(&self.inner.deps.db, f)
                .await
                .map_err(|e| e.to_string())?,
        );
        let total = rows.len();
        let mw = self.maxwin_ctx();
        let facts = lk(&self.inner.deals);
        // Решение 197: у записи сделки — её факт из учёта (итог, вложено), чтобы окно показывало план против факта.
        let page: Vec<Value> = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(id, b)| {
                let body = basis(serde_json::from_str::<Value>(&b).unwrap_or_default());
                let deal = body["battle"]["deal_id"].as_str().and_then(|d| facts.get(d));
                let fact = deal.map(|d| json!({"result_usd": d.result_usd, "invested": d.invested, "started_at": d.started_at}));
                // Решение 245: у кандидата — окно максимального объёма и «успели бы».
                let view = (body["battle"]["verdict"] != "deal").then(|| mw.of_body(&body));
                json!({"id": id, "body": body, "fact": fact, "maxwin_view": view})
            })
            .collect();
        drop(facts);
        Ok(json!({"rows": page, "total": total}))
    }

    /// Одна запись по id (None — нет). Ошибки: текст сбоя базы.
    pub async fn uncapped_get(&self, id: i64) -> Result<Option<Value>, String> {
        let b = uncapped_db::get(&self.inner.deps.db, id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(b.map(|b| basis(serde_json::from_str(&b).unwrap_or_default())))
    }

    /// Запись сделки по её id (None — не посчитана или фича выключена). Ошибки: текст сбоя базы.
    pub async fn uncapped_deal(&self, deal_id: &str) -> Result<Option<Value>, String> {
        let r = uncapped_db::by_deal(&self.inner.deps.db, deal_id.to_string())
            .await
            .map_err(|e| e.to_string())?;
        Ok(r.map(|(id, b)| json!({"id": id, "body": basis(serde_json::from_str::<Value>(&b).unwrap_or_default())})))
    }

    /// Последние записи всех сделок: {id сделки: {id, body}} — одним запросом (экран «Сделки»). Ошибки: база (текст).
    pub async fn uncapped_deals(&self) -> Result<Value, String> {
        let rows = uncapped_db::by_deals(&self.inner.deps.db)
            .await
            .map_err(|e| e.to_string())?;
        let map: serde_json::Map<String, Value> = rows
            .into_iter()
            .map(|(deal, id, b)| {
                let body = basis(serde_json::from_str::<Value>(&b).unwrap_or_default());
                (deal, json!({"id": id, "body": body}))
            })
            .collect();
        Ok(Value::Object(map))
    }

    /// Почему расчёт без потолка не идёт (None — включён). Ошибок нет.
    pub fn uncapped_off(&self) -> Option<String> {
        match self
            .inner
            .deps
            .settings
            .flag("uncapped.enabled", Default::default())
        {
            Ok(true) => None,
            Ok(false) => Some("uncapped.enabled = нет".into()),
            Err(e) => Some(e.message),
        }
    }

    /// Счётчики очереди и среднее время расчёта. Ошибок нет.
    fn uncapped_stats(&self) -> Value {
        let st = &self.inner.uncapped;
        let mut s = lk(&st.stats).clone();
        s.waiting = st.waiting.load(Ordering::Acquire) as u64;
        s.deals_waiting = st.deals_waiting.load(Ordering::Acquire) as u64;
        s.running = lk(&st.slots).values().filter(|x| x.running).count() as u64;
        let mean = if s.done > 0 {
            s.total_ms / s.done as f64
        } else {
            0.0
        };
        json!({"counters": s, "mean_ms": mean})
    }
}

/// Решение 217: у записи без поля basis (до решения 217) — источник «после покупки» (uncapped_types::old_basis), чтобы
/// экран отличал занижённые записи сделок. Принимает: тело записи. Возвращает: его же с basis. Ошибок нет.
fn basis(mut body: Value) -> Value {
    if let Some(o) = body.as_object_mut()
        && !o.contains_key("basis")
    {
        o.insert(
            "basis".into(),
            Value::String(super::uncapped_types::old_basis()),
        );
    }
    body
}
