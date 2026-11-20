//! Файл: venues/bsc_dex/exec/executor/live_proof.rs
//! Что делает: решение 400 (t185) — приёмка исполнителя по классам шага пулов менеджеров (вид 19 / 20 / 21 / 23 ×
//! крючок, и Infinity Stable) из записи симуляции на живых пулах, вшитой в сборку. Решение 461 (T457-02): записи — по
//! ключу (версия кода, chainId сети), exec/executor/live_record.rs (v3 — BSC 2026-11-17, v4 — Arbitrum 2026-11-20);
//! кошелёк судится записью версии своего исполнителя и сети своей площадки DEX (dex.evm_chain_id); записи для пары нет —
//! все классы «не проверен» (шаги менеджеров не идут). Правило — exec/executor/live_rule.rs. Здесь:
//! - verdicts_of: вердикты по классам для (версия, сеть);
//! - accepted_for / admits_for: исполним ли шаг пула менеджера (одно правило для котировки и сделки: pool_gate.rs —
//!   его зовут entry.rs fits / singleton, quote/graph_executor.rs, quote/graph_fit.rs, exec/executor/mod.rs takes,
//!   exec/cycle_gate.rs, check/probe_exec.rs, call::build; check/pass_cycle.rs);
//! - listing_of / proofs: что принято и что нет — текст причины запрета развёртывания (deploy_gate.rs), строка сверки
//!   исполнителя (onchain.rs), строки окна кошельков и развёртывания (pass_view.rs, deploy.rs).
//! Решение 401: admits_for зовёт exec/executor/pool_gate.rs — к классу добавлен факт пробы пула через исполнитель кошелька.
//! Кто вызывает: exec/executor/{pool_gate, pass_view, deploy_gate, deploy, onchain}.rs, check/pass_cycle.rs,
//! pools/exec_kind.rs (describe), тесты.
//! От чего зависит: exec/executor/{code, live_rule, live_record}.rs, pools/exec_kind.rs, core/types/executor.rs, ctx.rs.
//! Настройки: dex.evm_chain_id [bsc_dex] — сеть площадки кошелька (читается, не заводится; решение 416).

use std::sync::OnceLock;

use alloy_primitives::Address;

use crate::core::types::ExecutorClassProof;

use super::super::super::ctx::Ctx;
use super::super::super::pools::exec_kind::ExecKind;
use super::code::{self, Version};
use super::live_record;
pub use super::live_record::{Verdicts, pair};
use super::live_rule::{Class, Hook};

/// Сеть площадки DEX окружения (dex.evm_chain_id). None — не задана: записи для такой сети нет. Ошибок нет.
pub fn chain_of(ctx: &Ctx) -> Option<u64> {
    ctx.int("dex.evm_chain_id").ok()
}

/// Каталог записи пары (для текстов): None — записи нет. Ошибок нет.
pub fn record_of(v: Version, chain: Option<u64>) -> Option<&'static str> {
    live_record::find(v, chain).map(|j| j.dir)
}

/// Вердикты записи пары (версия кода, сеть кошелька). Запись есть — её вердикты (запись сверена с SHA-256 bin-runtime
/// этой версии, code::runtime_sha256_of); записи нет — все классы «не проверен» (Ok, ни один не принят); у v1 / v2 шагов
/// менеджеров нет. Ошибки (текст): запись не разобрана, сеть записи не прочитана, у версии шагов нет.
pub fn verdicts_of(v: Version, chain: Option<u64>) -> &'static Verdicts {
    static NONE: OnceLock<Verdicts> = OnceLock::new();
    if matches!(v, Version::V1 | Version::V2) {
        return NONE.get_or_init(|| Err("у исполнителя v1 / v2 шагов менеджеров нет".into()));
    }
    if let Some(j) = live_record::find(v, chain) {
        return &j.verdicts;
    }
    if let Some(j) = live_record::broken(v) {
        return &j.verdicts;
    }
    live_record::missing(v, chain)
}

/// Вердикты текущей сборки contracts/out/ (её разворачивает кнопка) в сети кошелька. Ошибки (текст): артефакт не
/// сверен, запись не разобрана.
pub fn verdicts_current(chain: Option<u64>) -> Result<(Version, &'static Verdicts), String> {
    code::current_version().map(|v| (v, verdicts_of(v, chain)))
}

/// Номер вида шага контракта у шага пула менеджера (kind.rs Params::kind: UNI_V4 = 19, INF_CL = 20, INF_BIN = 21,
/// решение 441 — BALANCER_V3 = 23); None — не шаг менеджера (решению 400 не подлежит). Ошибок нет.
pub fn number(k: ExecKind) -> Option<u8> {
    match k {
        ExecKind::UniV4 => Some(19),
        ExecKind::InfCl => Some(20),
        ExecKind::InfBin => Some(21),
        ExecKind::BalancerV3 => Some(23),
        _ => None,
    }
}

/// Класс шага пула. Принимает: вид шага, пул Infinity Stable ли, крючок ключа пула. None — не шаг менеджера. Ошибок нет.
pub fn class_of(k: ExecKind, stable: bool, hooks: Address) -> Option<Class> {
    let hook = if stable {
        Hook::Stable
    } else if hooks == Address::ZERO {
        Hook::Without
    } else {
        Hook::With
    };
    number(k).map(|kind| Class { kind, hook })
}

/// Горячий путь (решение 401, ревью t186 Н1): принят ли класс шага записью пары — без строк (вердикты разобраны один
/// раз). Принимает: версию исполнителя кошелька, сеть, вид шага, Infinity Stable ли, крючок. Ошибок нет.
pub fn accepted_for(
    v: Version,
    chain: Option<u64>,
    k: ExecKind,
    stable: bool,
    hooks: Address,
) -> bool {
    let Some(c) = class_of(k, stable, hooks) else {
        return true;
    };
    let Some(j) = live_record::find(v, chain) else {
        return false;
    };
    match &j.verdicts {
        Ok(list) => list.iter().any(|x| x.class == c && x.accepted),
        Err(_) => false,
    }
}

/// Исполним ли шаг по записи пары — с причиной. Возвращает: Ok — не шаг менеджера или класс принят; Err — «… у
/// исполнителя v4 в сети chainId 42161 не принят на живых пулах: …». Ошибок иных нет.
pub fn admits_for(
    v: Version,
    chain: Option<u64>,
    k: ExecKind,
    stable: bool,
    hooks: Address,
) -> Result<(), String> {
    let Some(c) = class_of(k, stable, hooks) else {
        return Ok(());
    };
    admits_in(v, chain, verdicts_of(v, chain), c)
}

/// То же по данным вердиктам (проверяемо тестом). Ошибок иных нет.
pub fn admits_in(v: Version, chain: Option<u64>, vs: &Verdicts, c: Class) -> Result<(), String> {
    let head = format!(
        "{} у исполнителя {} не принят на живых пулах",
        c.label(),
        pair(v, chain)
    );
    match vs {
        Err(e) => Err(format!("{head}: запись приёмки не разобрана — {e}")),
        Ok(list) => match list.iter().find(|x| x.class == c) {
            Some(x) if x.accepted => Ok(()),
            Some(x) => Err(format!("{head}: {}", x.why)),
            None => Err(format!("{head}: класса нет в правиле")),
        },
    }
}

/// Что принято и что нет — одной строкой. Принимает: версию, сеть, вердикты. Ошибок нет.
pub fn listing_of(v: Version, chain: Option<u64>, vs: &Verdicts) -> String {
    let at = match record_of(v, chain) {
        Some(dir) => format!("запись приёмки для версии {} — {dir}", pair(v, chain)),
        None => format!("записи приёмки для версии {} нет", pair(v, chain)),
    };
    let list = match vs {
        Err(e) => return format!("{at}: не разобрана — {e}"),
        Ok(l) => l,
    };
    let names = |ok: bool| {
        let n: Vec<String> = list
            .iter()
            .filter(|x| x.accepted == ok)
            .map(|x| x.class.label())
            .collect();
        if n.is_empty() {
            "нет".to_string()
        } else {
            n.join(", ")
        }
    };
    format!(
        "{at}; приняты на живых пулах: {}; не приняты: {}",
        names(true),
        names(false)
    )
}

/// listing_of для пары (сверка исполнителя кошелька, onchain.rs; описание вида, exec_kind.rs). Ошибок нет.
pub fn listing_for(v: Version, chain: Option<u64>) -> String {
    listing_of(v, chain, verdicts_of(v, chain))
}

/// Строки для интерфейса (окно кошельков / развёртывания) для пары. Запись не разобрана — пусто (причина — в запрете
/// развёртывания, deploy_gate.rs). Ошибок нет.
pub fn proofs(v: Version, chain: Option<u64>) -> Vec<ExecutorClassProof> {
    match verdicts_of(v, chain) {
        Err(_) => Vec::new(),
        Ok(list) => list
            .iter()
            .map(|x| ExecutorClassProof {
                class: x.class.label(),
                pools: x.pools,
                proven: x.proven,
                accepted: x.accepted,
                why: x.why.clone(),
                live: String::new(),
            })
            .collect(),
    }
}
