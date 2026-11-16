//! Файл: venues/bsc_dex/quote/usable_reuse.rs
//! Что делает: решение 336 (задача t157, п. 2а) — допуск пула для классики (quote/select.rs usable: достоверность,
//! глубина, DEX подключена, бан справочника, крючок, сверка с квотером, газ, налог пула) от объёма НЕ зависит, а
//! считался на каждой котировке каждой точки подбора объёма — около 15 чтений настроек и 4 общих замка на пул (разбор
//! docs/notes/t157-cpu-parallel-2026-11-16.md п. 1б). При dex.quote_reuse = да внутри подбора объёма кандидата он
//! считается один раз на пул и сторону и берётся из короба подбора (gateway/calc_share.rs; короб переносится в поток
//! счёта quote/cpu.rs → cpu_local.rs).
//! Свежесть: запись короба помнит поколение состояния пула (Entry.state_gen, pools/state_gen.rs), достоверность и
//! «спит»; не совпало с записью, по которой котируют сейчас, или поколение 0 (запись вне реестра) — допуск заново.
//! Ключ короба — всё, от чего допуск зависит кроме пула: сторона, decimals USDT, плановая цена газа (пулы Solidly,
//! ревью 273), снимок (frozen_head), продажа выхода (решение 247 п. В).
//! Что в пределах одного подбора НЕ видно (видно со следующего кандидата и в финальной котировке — она идёт без
//! короба): смена фактов проверок пула без смены его состояния (квотер, налог, крючок, газ), смена настроек допуска,
//! бан справочника. Так же, как у графа (quote/graph_reuse.rs).
//! Вне подбора (финальная котировка, исполнение, экран, проверки) короба нет — допуск каждый раз, как до 336.
//! dex.quote_reuse не задана или нет — как до 336.
//! Замер (решение 312, log.quote_timing): этап «в т.ч. классика: допуск пулов», счётчики допусков и попаданий.
//! Кто вызывает: quote/select.rs (usable).
//! От чего зависит: quote/select.rs (usable_with), quote/cpu_local.rs, gateway/calc_share.rs, gateway/exit_scope.rs,
//! quote/quote_timing.rs.
//! Настройки: dex.quote_reuse [bsc_dex] (необязательная; читается один раз на подбор).

use std::collections::HashMap;
use std::sync::RwLock;

use crate::core::types::{PoolRef, Side};

use super::super::pools::registry::Entry;
use super::Quoting;
use super::cpu_local;
use super::quote_timing::{self, Count, Stage};
use super::select::{self, Usable};

/// Допуск одного пула: на чём посчитан (поколение, достоверен, спит) и итог.
type Slot = (u64, bool, bool, Result<Usable, String>);

/// Допуски одного подбора при одном ключе; on — dex.quote_reuse на начало подбора.
struct Admits {
    on: bool,
    map: RwLock<HashMap<PoolRef, Slot>>,
}

/// Что делает: допуск пула (см. шапку). Принимает и возвращает: как select::usable. Ошибки: как select::usable.
pub fn usable(q: &Quoting, e: &Entry, side: Side, usdt_dec: u8) -> Result<Usable, String> {
    // Решение 336: замер — время и число допусков (часы только при замере котировки).
    let t = quote_timing::now();
    let (out, hit) = admit(q, e, side, usdt_dec);
    quote_timing::add(Stage::ClassicAdmit, t);
    quote_timing::count(Count::AdmitCalls, 1);
    quote_timing::count(Count::AdmitHits, u64::from(hit));
    out
}

/// Допуск пула и признак «взят из короба». Принимает: как usable. Ошибок сверх usable нет.
fn admit(q: &Quoting, e: &Entry, side: Side, usdt_dec: u8) -> (Result<Usable, String>, bool) {
    let fresh = || (select::usable_with(q, e, side, usdt_dec, None), false);
    let Some(share) = cpu_local::share() else {
        return fresh();
    };
    if e.state_gen == 0 {
        return fresh();
    }
    let exit = crate::gateway::exit_scope::is_exit();
    let key = format!(
        "bsc_dex.usable_reuse|{side:?}|{usdt_dec}|{:?}|{:?}|{exit}",
        q.plan_gas, q.frozen_head
    );
    let a = share.get_or_put(&key, || Admits {
        on: q.ctx.flag("dex.quote_reuse").unwrap_or(false),
        map: RwLock::new(HashMap::new()),
    });
    if !a.on {
        return fresh();
    }
    let r = &e.pool.reference;
    let hit = a
        .map
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .get(r)
        .filter(|s| s.0 == e.state_gen && s.1 == e.valid && s.2 == e.asleep)
        .map(|s| s.3.clone());
    if let Some(h) = hit {
        return (h, true);
    }
    let out = select::usable_with(q, e, side, usdt_dec, None);
    a.map
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert(r.clone(), (e.state_gen, e.valid, e.asleep, out.clone()));
    (out, false)
}
