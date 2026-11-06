//! Файл: core/uncapped_view.rs
//! Что делает: данные экрана «Упущено из-за потолка» (решение 141) за период: два итога — «по исполненным сделкам»
//! (боевая плановая прибыль, фактический итог сделки из учёта, прибыль той же моделью на нашем бюджете, потенциал без
//! потолка, упущено, сколько упёрлось в бюджет) и «по не исполненным кандидатам» (эпизоды: упущено — пиковое в эпизоде,
//! сколько отклонённых стали бы плюсовыми без потолка); таблица «потолок → прибыль» (значения — uncapped.ladder_usdt,
//! «взяли бы» = прибыль лучшего объёма в потолке, если она проходит пороги, иначе 0) и строка «без потолка»; топ
//! направлений по упущенному; счётчики очереди (посчитано, слито, не посчитано, сбои, мс на расчёт). Список записей
//! периода страницей, одна запись, запись сделки по её id.
//! Кто вызывает: api/uncapped.rs.
//! От чего зависит: storage/uncapped_db.rs, uncapped_types.rs, uncapped_state.rs, uncapped.rs (ladder), ledger (сделки).
//! Настройки: uncapped.enabled, uncapped.ladder_usdt (только показ и причина).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::storage::uncapped_db::{self, Filter};

use super::engine::{Core, lk};
use super::uncapped_types::{Brief, budget_bound, take};

/// Сумма по группе записей.
#[derive(Default)]
struct Sum {
    count: u64,
    bound: u64,
    battle: Decimal,
    at_budget: Decimal,
    potential: Decimal,
    missed: Decimal,
    fact: Decimal,
    fact_count: u64,
    plus_uncapped: u64,
}

impl Sum {
    /// Добавляет запись (fact — итог сделки, если известен). Ошибок нет.
    fn add(&mut self, b: &Brief, fact: Option<Decimal>) {
        self.count += 1;
        self.bound += u64::from(budget_bound(&b.limited_by));
        self.battle += b.battle.profit;
        self.at_budget += take(&b.at_budget);
        if b.opt_passes {
            self.potential += b.opt.profit;
        }
        self.missed += b.missed;
        if let Some(f) = fact {
            self.fact += f;
            self.fact_count += 1;
        }
        self.plus_uncapped += u64::from(b.battle.verdict == "reject" && b.opt_passes);
    }

    /// JSON для экрана. Ошибок нет.
    fn json(&self) -> Value {
        json!({
            "count": self.count, "budget_bound": self.bound, "battle_profit": self.battle,
            "at_budget_profit": self.at_budget, "potential_profit": self.potential, "missed": self.missed,
            "fact": self.fact, "fact_count": self.fact_count, "plus_uncapped": self.plus_uncapped,
        })
    }
}

impl Core {
    /// Что делает: итоги за период [from, to) (RFC 3339; None — без границы), топ `top` направлений.
    /// Возвращает: {off, deals, candidates, ladder, top, stats}. Ошибки: текст сбоя базы.
    pub async fn uncapped_summary(
        &self,
        from: Option<String>,
        to: Option<String>,
        top: usize,
    ) -> Result<Value, String> {
        let f = Filter {
            from,
            to,
            kind: None,
        };
        let rows = uncapped_db::all(&self.inner.deps.db, f)
            .await
            .map_err(|e| e.to_string())?;
        let (ladder, ladder_err) = match super::uncapped::ladder(self) {
            Ok(l) => (l, None),
            Err(e) => (vec![], Some(e)),
        };
        let (mut deals, mut cands) = (Sum::default(), Sum::default());
        let mut lad = vec![[Decimal::ZERO; 2]; ladder.len()];
        let mut by_dir: HashMap<String, (Decimal, u64)> = HashMap::new();
        let facts = lk(&self.inner.deals);
        for (_, body) in &rows {
            let Ok(b) = serde_json::from_str::<Brief>(body) else {
                continue;
            };
            let g = usize::from(b.battle.verdict != "deal");
            match b.battle.deal_id.as_ref() {
                Some(id) if g == 0 => deals.add(&b, facts.get(id).and_then(|d| d.result_usd)),
                _ => cands.add(&b, None),
            }
            for (i, cap) in ladder.iter().enumerate() {
                if let Some(l) = b.levels.iter().find(|l| l.cap == *cap) {
                    lad[i][g] += take(l);
                }
            }
            let d = &b.direction;
            let key = format!("{} {} → {}", d.asset.ticker, d.buy_on.0, d.sell_on.0);
            let e = by_dir.entry(key).or_default();
            e.0 += b.missed;
            e.1 += 1;
        }
        drop(facts);
        let mut top_v: Vec<_> = by_dir.into_iter().collect();
        top_v.sort_by_key(|b| std::cmp::Reverse(b.1.0));
        top_v.truncate(top);
        let ladder_rows: Vec<Value> = ladder
            .iter()
            .zip(&lad)
            .map(|(c, s)| json!({"cap": c, "deals": s[0], "candidates": s[1]}))
            .collect();
        Ok(json!({
            "off": self.uncapped_off(),
            "deals": deals.json(),
            "candidates": cands.json(),
            "ladder": ladder_rows,
            "ladder_error": ladder_err,
            "uncapped": {"deals": deals.potential, "candidates": cands.potential},
            "top": top_v.iter().map(|(k, (m, n))| json!({"direction": k, "missed": m, "count": n})).collect::<Vec<_>>(),
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
        let rows = uncapped_db::all(&self.inner.deps.db, f)
            .await
            .map_err(|e| e.to_string())?;
        let total = rows.len();
        let page: Vec<Value> = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(id, b)| json!({"id": id, "body": serde_json::from_str::<Value>(&b).unwrap_or_default()}))
            .collect();
        Ok(json!({"rows": page, "total": total}))
    }

    /// Одна запись по id (None — нет). Ошибки: текст сбоя базы.
    pub async fn uncapped_get(&self, id: i64) -> Result<Option<Value>, String> {
        let b = uncapped_db::get(&self.inner.deps.db, id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(b.map(|b| serde_json::from_str(&b).unwrap_or_default()))
    }

    /// Запись сделки по её id (None — не посчитана или фича выключена). Ошибки: текст сбоя базы.
    pub async fn uncapped_deal(&self, deal_id: &str) -> Result<Option<Value>, String> {
        let r = uncapped_db::by_deal(&self.inner.deps.db, deal_id.to_string())
            .await
            .map_err(|e| e.to_string())?;
        Ok(r.map(|(id, b)| json!({"id": id, "body": serde_json::from_str::<Value>(&b).unwrap_or_default()})))
    }

    /// Последние записи всех сделок: {id сделки: {id, body}} — одним запросом (экран «Сделки»). Ошибки: база (текст).
    pub async fn uncapped_deals(&self) -> Result<Value, String> {
        let rows = uncapped_db::by_deals(&self.inner.deps.db)
            .await
            .map_err(|e| e.to_string())?;
        let map: serde_json::Map<String, Value> = rows
            .into_iter()
            .map(|(deal, id, b)| {
                let body = serde_json::from_str::<Value>(&b).unwrap_or_default();
                (deal, json!({"id": id, "body": body}))
            })
            .collect();
        Ok(Value::Object(map))
    }

    /// Почему расчёт без потолка не идёт (None — включён). Ошибок нет.
    fn uncapped_off(&self) -> Option<String> {
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
        s.running = lk(&st.slots).values().filter(|x| x.running).count() as u64;
        let mean = if s.done > 0 {
            s.total_ms / s.done as f64
        } else {
            0.0
        };
        json!({"counters": s, "mean_ms": mean})
    }
}
