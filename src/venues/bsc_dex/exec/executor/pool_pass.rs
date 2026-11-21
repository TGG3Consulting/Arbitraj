//! Файл: venues/bsc_dex/exec/executor/pool_pass.rs
//! Что делает: решение 401 (t186) — факт пула менеджера (Uniswap V4, Infinity CL / Bin / Stable) «пускает ли его
//! исполнитель v3 ЭТОГО кошелька»: по адресу исполнителя и пулу — прошёл обе стороны (блок пробы), отвергнут (текст
//! отката; пробой цикла крючков или откатом пробы сделки перед отправкой), не проверено (почему). Время проверки — у
//! каждого факта. Факт пишет проба цикла крючков (check/pass_cycle.rs, перепроверка — hooks.recheck_interval_s) и откат
//! пробы execute перед сделкой (on_sim: exec/executor/sim.rs, exec/cycle_exec.rs); читает правило исполнимости шага
//! (exec/executor/pool_gate.rs → entry.rs fits / singleton) и экран (pools/depth_facts.rs, pass_view.rs).
//! Хранится только в памяти (как итоги проверок, check/state.rs): после запуска фактов нет — до первой пробы действует
//! приёмка класса по записи (решение 400). Ключ — адрес исполнителя: крючок с белым списком смотрит msg.sender, и факт
//! одного исполнителя к другому не переносится; переразвёрнутый исполнитель (новый адрес) начинает без фактов.
//! Ревью t186 (Н1): горячий путь котировки читает только вид итога (kind) из снимка за ArcSwap — без замка и без копии
//! строк; полные факты (причина, время, блок) — в отдельной памяти под Mutex, её берут только писатели (проба, откат
//! сделки) и текст причины / экран. Снимок видов пересобирается, только когда вид итога пула сменился.
//! Ревью t186 (Н3): откат пробы сделки помечает только пул, крючок которого назван в WrappedError отката.
//! Кто вызывает: check/pass_cycle.rs, exec/executor/{pool_gate, sim, call, pass_view}.rs, exec/cycle_exec.rs,
//! pools/depth_facts.rs, тесты.
//! От чего зависит: check/{simulate, pass_judge}.rs, exec/executor/{call, code, remeasure}.rs, core/types
//! (PoolRef), settings/accounts (Wallet), arc-swap.
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use alloy_primitives::Address;
use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};

use crate::core::types::PoolRef;
use crate::settings::accounts::Wallet;

use super::super::super::check::pass_judge;
use super::super::super::check::simulate::CallResult;
use super::call::ExecCall;
use super::code::Version;

/// Итог проверки пула через исполнитель кошелька.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Проба через исполнитель прошла обе стороны (покупка и продажа), совпала с квотером, остаток на исполнителе 0.
    Passed,
    /// Исполнитель отвергнут: текст отката или расхождения; deal = true — откат пробы сделки перед отправкой (пул
    /// перепроверяется ближайшим проходом цикла крючков).
    Rejected { why: String, deal: bool },
    /// Не проверено (сверить не с чем, налог, нет источника пробы, сбой узла) — почему.
    Unchecked(String),
}

/// Факт пула по исполнителю.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub state: State,
    /// Когда записан.
    pub at: DateTime<Utc>,
    /// Блок, поверх которого шла проба (None — откат сделки или проба не дошла до узла).
    pub block: Option<u64>,
}

/// Вид итога (без причины) — для горячего пути котировки (pool_gate.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Passed,
    Rejected,
    Unchecked,
}

impl State {
    /// Вид итога. Ошибок нет.
    pub fn to_kind(&self) -> Kind {
        match self {
            State::Passed => Kind::Passed,
            State::Rejected { .. } => Kind::Rejected,
            State::Unchecked(_) => Kind::Unchecked,
        }
    }
}

/// Пул шага менеджера в вызове исполнителя (exec/executor/call.rs build): ссылка и крючок ключа (ревью t186, Н3: по
/// нему откат пробы сделки находит пул).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gated {
    pub pool: PoolRef,
    pub hooks: Address,
}

/// Решение 435 (433-и): ключ — (площадка DEX, исполнитель): исполнитель с тем же адресом (CREATE2 / тот же nonce) в
/// другой сети — другой контракт, его пробы не смешиваются.
type Key = (&'static str, Address);
/// Полные факты: (площадка, исполнитель) → пул → факт. Только писатели и текст / экран.
type Facts = HashMap<Key, HashMap<PoolRef, Fact>>;
/// Виды итогов: (площадка, исполнитель) → пул → вид. Снимок для горячего пути.
type Kinds = HashMap<Key, HashMap<PoolRef, Kind>>;

static FACTS: LazyLock<Mutex<Facts>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static KINDS: LazyLock<ArcSwap<Kinds>> = LazyLock::new(|| ArcSwap::from_pointee(HashMap::new()));

/// Горячий путь (ревью t186, Н1): вид итога пула по исполнителю — из снимка без замка и без копий. None — проверок не
/// было. Принимает: площадку DEX (ctx.place.id), исполнитель, пул. Ошибок нет.
pub fn kind(venue: &'static str, executor: Address, pool: &PoolRef) -> Option<Kind> {
    KINDS.load().get(&(venue, executor))?.get(pool).copied()
}

/// Полный факт (причина, время, блок) — для текста причины, экрана и цикла проб (не горячий путь: Mutex). None —
/// проверок не было. Принимает: площадку DEX, исполнитель, пул. Ошибок нет.
pub fn get(venue: &'static str, executor: Address, pool: &PoolRef) -> Option<Fact> {
    let m = FACTS.lock().unwrap_or_else(|p| p.into_inner());
    m.get(&(venue, executor))?.get(pool).cloned()
}

/// Записывает факт; вид итога сменился — новый снимок видов. Принимает: площадку DEX, исполнитель, пул, факт.
/// Возвращает: сменился ли итог (вид или текст причины) — для журнала «только при смене». Ошибок нет.
pub fn put(venue: &'static str, executor: Address, pool: &PoolRef, fact: Fact) -> bool {
    let mut m = FACTS.lock().unwrap_or_else(|p| p.into_inner());
    let kind_now = fact.state.to_kind();
    let prev = m
        .entry((venue, executor))
        .or_default()
        .insert(pool.clone(), fact.clone());
    let kind_was = prev.as_ref().map(|p| p.state.to_kind());
    if kind_was != Some(kind_now) {
        // Под замком писателей: снимки не теряют друг друга.
        let mut next: Kinds = (**KINDS.load()).clone();
        next.entry((venue, executor))
            .or_default()
            .insert(pool.clone(), kind_now);
        KINDS.store(Arc::new(next));
    }
    drop(m);
    prev.is_none_or(|p| p.state != fact.state)
}

/// Решение 401, ревью t186 (Н3): откат пробы execute перед отправкой сделки помечает «отвергнут» до следующей
/// перепроверки ТОЛЬКО пул, на котором откат произошёл: WrappedError отката называет крючок (check/pass_judge.rs
/// hook_revert), и помечается пул шага с этим крючком. Откат без WrappedError крючка пула вызова (другой шаг маршрута,
/// цена, состояние) — не помечается никто. Не помечает и успех, срок, minOut (решение 247). Принимает: площадку DEX
/// (решение 435), вызов, итог пробы. Возвращает: помеченные пулы. Ошибок нет; синхронно — без await на пути сделки.
pub fn on_sim(venue: &'static str, c: &ExecCall, r: &CallResult) -> Vec<PoolRef> {
    if r.ok() || pass_judge::late(r) || super::remeasure::is_min_out(r) {
        return Vec::new();
    }
    let marked: Vec<&Gated> = c
        .gated
        .iter()
        .filter(|g| pass_judge::hook_revert(r, g.hooks).is_some())
        .collect();
    if marked.is_empty() {
        return Vec::new();
    }
    let why = format!(
        "откат пробы сделки перед отправкой: {}",
        pass_judge::revert_text(r)
    );
    for g in &marked {
        let fact = Fact {
            state: State::Rejected {
                why: why.clone(),
                deal: true,
            },
            at: Utc::now(),
            block: None,
        };
        put(venue, c.executor, &g.pool, fact);
    }
    tracing::warn!(
        executor = %c.executor,
        pools = marked.len(),
        "решение 401: пулы помечены «исполнитель отвергнут» до перепроверки — {why}"
    );
    marked.into_iter().map(|g| g.pool.clone()).collect()
}

/// Адрес исполнителя v3 кошелька (сверен: версия «v3» у записи кошелька — exec/executor/onchain.rs пишет после сверки
/// кода). None — исполнителя нет или не v3 (пробы через него не идут). Ошибок нет.
pub fn v3(w: &Wallet) -> Option<Address> {
    let v = w.executor_version.as_deref().and_then(Version::parse);
    // Решение 441: v4 — надмножество v3.
    if !matches!(v, Some(Version::V3 | Version::V4 | Version::V5)) {
        return None;
    }
    w.executor.as_deref()?.parse().ok()
}

/// Строка факта для экрана и причин. Принимает: имя кошелька, факт (None — не было). Ошибок нет.
pub fn line(wallet: &str, fact: Option<&Fact>) -> String {
    let head = format!("исполнитель v3 кошелька «{wallet}»");
    let Some(f) = fact else {
        return format!(
            "{head}: не проверено (проба пула через исполнитель ещё не проводилась, решение 401)"
        );
    };
    let when = match f.block {
        Some(b) => format!("{} UTC, блок {b}", f.at.format("%d.%m %H:%M:%S")),
        None => format!("{} UTC", f.at.format("%d.%m %H:%M:%S")),
    };
    match &f.state {
        State::Passed => format!("{head}: проходит обе стороны ({when})"),
        State::Rejected { why, .. } => format!("{head}: отвергнут ({when}): {why}"),
        State::Unchecked(why) => format!("{head}: не проверено ({when}): {why}"),
    }
}
