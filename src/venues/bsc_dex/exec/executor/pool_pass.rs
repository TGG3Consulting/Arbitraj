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
//! Кто вызывает: check/pass_cycle.rs, exec/executor/{pool_gate, sim, call, pass_view}.rs, exec/cycle_exec.rs,
//! pools/depth_facts.rs, тесты.
//! От чего зависит: check/{simulate, pass_judge}.rs, exec/executor/{call, code, remeasure}.rs, core/types
//! (PoolRef), settings/accounts (Wallet).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use alloy_primitives::Address;
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

/// Пул шага менеджера в вызове исполнителя (exec/executor/call.rs build): ссылка, крючок ключа и признак «исполним
/// только по факту пробы этого пула» (класс не принят записью — решение 400).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gated {
    pub pool: PoolRef,
    pub hooks: Address,
    pub by_pass: bool,
}

/// Факты: исполнитель → пул → факт (вложенно: чтение на пути котировки без копии ссылки пула).
type Store = HashMap<Address, HashMap<PoolRef, Fact>>;

static FACTS: LazyLock<RwLock<Store>> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Факт пула по исполнителю. Принимает: адрес исполнителя, пул. None — проверок не было. Ошибок нет.
pub fn get(executor: Address, pool: &PoolRef) -> Option<Fact> {
    let m = FACTS.read().unwrap_or_else(|p| p.into_inner());
    m.get(&executor)?.get(pool).cloned()
}

/// Записывает факт. Принимает: исполнитель, пул, факт. Возвращает: сменился ли итог (вид или текст причины) — для журнала
/// «только при смене» и роста поколения итогов проверок. Ошибок нет.
pub fn put(executor: Address, pool: &PoolRef, fact: Fact) -> bool {
    let mut m = FACTS.write().unwrap_or_else(|p| p.into_inner());
    let prev = m
        .entry(executor)
        .or_default()
        .insert(pool.clone(), fact.clone());
    prev.is_none_or(|p| p.state != fact.state)
}

/// Решение 401: откат пробы execute перед отправкой сделки помечает пул «отвергнут» до следующей перепроверки, чтобы бот не
/// натыкался на него снова. Не помечает: успех, срок транзакции вышел, откат по minOut (решение 247 — свойство суммы, не
/// пула). Какой пул: откат крючка (WrappedError с адресом крючка, check/pass_judge.rs hook_revert) — пулы шага с этим
/// крючком; иначе — пулы, исполнимые только по факту пробы (by_pass: их класс записью не принят). Пулы принятых записью
/// классов при откате без крючка не помечаются (причина отката им не приписывается). Принимает: вызов, итог пробы.
/// Возвращает: помеченные пулы (для журнала вызывающего). Ошибок нет; синхронно — без await на пути сделки.
pub fn on_sim(c: &ExecCall, r: &CallResult) -> Vec<PoolRef> {
    if r.ok() || pass_judge::late(r) || super::remeasure::is_min_out(r) {
        return Vec::new();
    }
    let by_hook: Vec<&Gated> = c
        .gated
        .iter()
        .filter(|g| pass_judge::hook_revert(r, g.hooks).is_some())
        .collect();
    let marked: Vec<&Gated> = if by_hook.is_empty() {
        c.gated.iter().filter(|g| g.by_pass).collect()
    } else {
        by_hook
    };
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
        put(c.executor, &g.pool, fact);
    }
    if !marked.is_empty() {
        tracing::warn!(
            executor = %c.executor,
            pools = marked.len(),
            "решение 401: пулы помечены «исполнитель отвергнут» до перепроверки — {why}"
        );
    }
    marked.into_iter().map(|g| g.pool.clone()).collect()
}

/// Адрес исполнителя v3 кошелька (сверен: версия «v3» у записи кошелька — exec/executor/onchain.rs пишет после сверки
/// кода). None — исполнителя нет или не v3 (пробы через него не идут). Ошибок нет.
pub fn v3(w: &Wallet) -> Option<Address> {
    let v = w.executor_version.as_deref().and_then(Version::parse);
    if v != Some(Version::V3) {
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
