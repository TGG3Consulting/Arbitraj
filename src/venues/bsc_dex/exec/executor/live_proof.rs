//! Файл: venues/bsc_dex/exec/executor/live_proof.rs
//! Что делает: решение 400 (t185) — приёмка исполнителя v3 по классам шага (вид 19 / 20 / 21 × без крючка / с крючком,
//! и Infinity Stable) из записи симуляции на живых пулах сети, вшитой в сборку (include_str): каталог
//! tests/data/executor_v3_live_2026-11-17/ — README.md (SHA-256 симулированного bin-runtime), summary.json (по вызову),
//! hooks_reject.json (откаты крючков). Правило — exec/executor/live_rule.rs. Здесь:
//! - verdicts: вердикты по классам для кода v3 (решение 441, ревью п. 1: SHA-256 bin-runtime той версии, что у
//!   кошелька — verdicts_of; v3 — contracts/out/v3/ArbExecutor.sha256), один раз за запуск на версию;
//! - admits: исполним ли шаг пула менеджера исполнителем v3 — только если его класс принят (одно правило для котировки
//!   и сделки: exec/executor/entry.rs fits и singleton — их зовут quote/graph_executor.rs all_via, quote/graph_fit.rs,
//!   exec/executor/mod.rs takes, exec/cycle_gate.rs, check/probe_exec.rs, call::build);
//! - listing / proofs: что принято и что нет — текст причины запрета развёртывания (deploy_gate.rs), строка сверки
//!   исполнителя (onchain.rs), строки окна кошельков (venue.rs → api/wallets_view.rs → интерфейс).
//!
//! Новая запись (пересборка контракта, новые классы) — новый каталог tests/data и путь здесь, в том же коммите.
//! Решение 401: admits зовёт exec/executor/pool_gate.rs — к классу добавлен факт пробы пула через исполнитель кошелька.
//! Кто вызывает: exec/executor/{pool_gate, pass_view, deploy_gate, deploy, onchain}.rs, check/pass_cycle.rs,
//! pools/exec_kind.rs (describe), venue.rs, тесты.
//! От чего зависит: exec/executor/{code, live_rule}.rs, pools/exec_kind.rs, core/types/executor.rs.
//! Настройки: нет (факт о собранном коде, решение 400).

use std::sync::OnceLock;

use alloy_primitives::Address;

use crate::core::types::ExecutorClassProof;

use super::super::super::pools::exec_kind::ExecKind;
use super::code::{self, Version};
use super::live_rule::{self, Class, Hook, Verdict};

/// Каталог записи (для текстов и тестов).
pub const RECORD: &str = "tests/data/executor_v3_live_2026-11-17";

const README: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/executor_v3_live_2026-11-17/README.md"
));
const SUMMARY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/executor_v3_live_2026-11-17/summary.json"
));
const REJECT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/executor_v3_live_2026-11-17/hooks_reject.json"
));

/// Вердикты записи для кода версии v3 (порядок live_rule::CLASSES) — версии, к которой относится запись 17.11 и которая
/// стоит у кошельков. Ошибки (текст): запись не разобрана.
pub fn verdicts() -> &'static Result<Vec<Verdict>, String> {
    verdicts_of(Version::V3)
}

/// Решение 441 (ревью, п. 1): вердикты записи для кода данной версии — запись сверяется с SHA-256 bin-runtime той
/// версии, что стоит у кошелька (code::runtime_sha256_of: v3 — out/v3/, v4 — out/). После пересборки v4 записи о коде
/// v4 нет — у кошелька с v4 классы не приняты («запись о другом коде»), у кошельков с v3 — как до пересборки. У v1 / v2
/// шагов менеджеров нет. Один разбор на версию за запуск. Ошибки (текст): запись не разобрана, у версии шагов нет.
pub fn verdicts_of(v: Version) -> &'static Result<Vec<Verdict>, String> {
    static V3: OnceLock<Result<Vec<Verdict>, String>> = OnceLock::new();
    static V4: OnceLock<Result<Vec<Verdict>, String>> = OnceLock::new();
    static NONE: OnceLock<Result<Vec<Verdict>, String>> = OnceLock::new();
    let cell = match v {
        Version::V3 => &V3,
        Version::V4 => &V4,
        Version::V1 | Version::V2 => {
            return NONE.get_or_init(|| Err("у исполнителя v1 / v2 шагов менеджеров нет".into()));
        }
    };
    cell.get_or_init(|| live_rule::judge(README, SUMMARY, REJECT, code::runtime_sha256_of(v)))
}

/// Вердикты для текущей сборки contracts/out/ (её разворачивает кнопка; до пересборки — v3, после — v4). Ошибки
/// (текст): артефакт не сверен, запись не разобрана.
pub fn verdicts_current() -> &'static Result<Vec<Verdict>, String> {
    static BAD: OnceLock<Result<Vec<Verdict>, String>> = OnceLock::new();
    match code::current_version() {
        Ok(v) => verdicts_of(v),
        Err(e) => BAD.get_or_init(|| Err(e)),
    }
}

/// Номер вида шага контракта у шага пула менеджера (kind.rs Params::kind: UNI_V4 = 19, INF_CL = 20, INF_BIN = 21,
/// решение 441 — BALANCER_V3 = 23);
/// None — не шаг менеджера (решению 400 не подлежит). Ошибок нет.
pub fn number(k: ExecKind) -> Option<u8> {
    match k {
        ExecKind::UniV4 => Some(19),
        ExecKind::InfCl => Some(20),
        ExecKind::InfBin => Some(21),
        // Решение 441 (ревью, п. 2): шаг BALANCER_V3 — тоже только по записи о приёмке на живых пулах (её пока нет).
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

/// Решение 400: исполним ли шаг исполнителем v3. Принимает: вид шага, Infinity Stable ли, крючок ключа. Возвращает:
/// Ok — не шаг менеджера или класс принят записью; Err — причина «… у v3 не принят на живых пулах: …». Ошибок иных нет.
pub fn admits(k: ExecKind, stable: bool, hooks: Address) -> Result<(), String> {
    let Some(c) = class_of(k, stable, hooks) else {
        return Ok(());
    };
    admits_in(verdicts(), c)
}

/// Решение 401, ревью t186 (Н1): то же, что `admits(..).is_ok()`, без строк — для горячего пути котировки
/// (pool_gate.rs decide). Вердикты — OnceLock (после первого разбора чтение без замка). Ошибок нет.
pub fn accepted(k: ExecKind, stable: bool, hooks: Address) -> bool {
    accepted_for(Version::V3, k, stable, hooks)
}

/// Решение 441 (ревью, п. 1): то же для кода данной версии исполнителя кошелька. Ошибок нет.
pub fn accepted_for(v: Version, k: ExecKind, stable: bool, hooks: Address) -> bool {
    let Some(c) = class_of(k, stable, hooks) else {
        return true;
    };
    match verdicts_of(v) {
        Ok(list) => list.iter().any(|x| x.class == c && x.accepted),
        Err(_) => false,
    }
}

/// То же по данным вердиктам (проверяемо тестом). Ошибок иных нет.
pub fn admits_in(v: &Result<Vec<Verdict>, String>, c: Class) -> Result<(), String> {
    let head = format!("{} у исполнителя v3 не принят на живых пулах", c.label());
    match v {
        Err(e) => Err(format!("{head}: запись {RECORD} не разобрана — {e}")),
        Ok(list) => match list.iter().find(|x| x.class == c) {
            Some(x) if x.accepted => Ok(()),
            Some(x) => Err(format!("{head}: {}", x.why)),
            None => Err(format!("{head}: класса нет в правиле")),
        },
    }
}

/// Что принято и что нет — одной строкой. Принимает: вердикты. Ошибок нет.
pub fn listing_of(v: &Result<Vec<Verdict>, String>) -> String {
    let list = match v {
        Err(e) => return format!("запись {RECORD} не разобрана — {e}"),
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
        "приняты на живых пулах (запись {RECORD}): {}; не приняты: {}",
        names(true),
        names(false)
    )
}

/// listing_of для кода v3 (кошельки с v3). Ошибок нет.
pub fn listing() -> String {
    listing_of(verdicts())
}

/// Решение 441 (ревью, п. 1): listing_of для кода данной версии (сверка исполнителя кошелька, onchain.rs). Ошибок нет.
pub fn listing_for(v: Version) -> String {
    listing_of(verdicts_of(v))
}

/// Строки для интерфейса (окно кошельков / развёртывания). Запись не разобрана — пусто (причина — в запрете
/// развёртывания, deploy_gate.rs). Ошибок нет.
pub fn proofs() -> Vec<ExecutorClassProof> {
    match verdicts() {
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
