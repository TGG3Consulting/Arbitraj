//! Файл: venues/bsc_dex/pools/fee_probe.rs
//! Что делает: решение 274 (защита от забытой настройки) — при проверке пулов V3 (виды с ABI Uniswap или PancakeSwap)
//! спрашивает фабрику DEX `swapFee(0x0, 0)`. Фабрика SheepDex (SpeFactory) отвечает — значит, пул берёт комиссию обмена
//! у фабрики (`ISpeFactory(factory).swapFee(recipient, fee)`, SpePool.sol L2333), а не fee() пула. Если при этом
//! dex.v3_fee_source не задана, пулы этой DEX не берутся с точной причиной: иначе бот считал бы по fee() пула и мог
//! торговать по заниженной комиссии. Фабрики Uniswap V3 и PancakeSwap V3 такой функции не имеют — вызов откатывается
//! (записано 14.11.2026, tests/data/sheepdex_2026-11-14/swap_fee_probe.json) — пулы берутся как раньше.
//! Ответ фабрики запоминается на (id DEX, адрес фабрики) до перезапуска: проба — один вызов на DEX при первой проверке
//! её пулов (во второй проход verify.rs вместе с реестром фабрики), на рабочий цикл (чтение пулов на блоке) не влияет.
//! Сбой узла не запоминается — пулы DEX «не проверены», повтор при следующей проверке.
//! Ревью 281 (В3): проба пропускается только при dex.v3_fee_source = factory_swap_fee (pool и pool_fee_each_read защиту
//! не обходят: фабрика отвечает — пулы не берутся); у DEX с dex.v3_registry_fee_method или dex.v3_fee_manager без
//! pool_fee_each_read пулы не берутся с причиной (комиссию меняет менеджер — fee() надо читать с каждым состоянием).
//! Кто вызывает: pools/verify.rs (contracts).
//! От чего зависит: ctx.rs, node/call.rs (Piece), families.rs, fee_override.rs (ISpeFactory, FACTORY), fee_fresh.rs
//! (EACH_READ), registry_key.rs (method).
//! Настройки: dex.v3_fee_source, dex.factory, dex.v3_registry_fee_method, dex.v3_fee_manager [DEX].
//!
//! Источник: SpeFactory.sol (Sourcify, 0x571521f8c16f3c4ed5f2490f19187ba7a5a3cbdf) — `swapFee` L2604-L2610;
//! IUniswapV3Factory (Uniswap v3-core) — функции swapFee нет.

use std::collections::HashMap;
use std::sync::Mutex;

use alloy::sol_types::SolCall;
use alloy_primitives::aliases::U24;
use alloy_primitives::{Address, Bytes};

use super::super::ctx::Ctx;
use super::super::node::call::Piece;
use super::families::{Family, Proto};
use super::fee_fresh::EACH_READ;
use super::fee_override::{FACTORY, ISpeFactory};
use super::verify::Verdict;

/// Ответ фабрики на swapFee по (id DEX, фабрика): true — отвечает (комиссия от фабрики), false — откат.
static SEEN: Mutex<Option<HashMap<(&'static str, Address), bool>>> = Mutex::new(None);

/// Итог пробы для DEX.
#[derive(Debug, Clone)]
enum Gate {
    /// Пулы DEX берутся как раньше.
    Open,
    /// Пулы DEX не берутся — причина.
    Shut(String),
    /// Узел не ответил — пулы «не проверены».
    Unknown(String),
}

/// Пробы одной проверки: итог по id DEX и порядок спрошенных DEX (как вызовы в пачке).
#[derive(Debug, Default)]
pub struct Probes {
    gates: HashMap<&'static str, Gate>,
    asked: Vec<(&'static Family, Address)>,
}

/// Что известно о фабрике из памяти. Ошибок нет.
fn seen(id: &'static str, factory: Address) -> Option<bool> {
    let g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(&(id, factory)).copied())
}

/// Запомнить ответ фабрики. Ошибок нет.
fn remember(id: &'static str, factory: Address, answers: bool) {
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .insert((id, factory), answers);
}

/// Причина «фабрика берёт комиссию через swapFee». Ошибок нет.
fn reason(f: &Family, factory: Address) -> String {
    format!(
        "{}: фабрика {factory:#x} берёт комиссию обмена через swapFee — задайте dex.v3_fee_source = {FACTORY} \
         (решение 274)",
        f.title
    )
}

/// Задана ли у DEX настройка комиссии от менеджера (решение 281): dex.v3_registry_fee_method или dex.v3_fee_manager.
/// Ошибок нет.
fn manager_set(ctx: &Ctx, f: &Family) -> bool {
    super::registry_key::method(ctx, f).is_some()
        || ctx.dex_addr("dex.v3_fee_manager", f.id).is_ok()
}

/// Причина «комиссию меняет менеджер, а fee() не перечитывается» (ревью 281, В3). Ошибок нет.
fn manager_reason(f: &Family) -> String {
    format!(
        "{}: комиссию пула меняет менеджер (задана dex.v3_registry_fee_method или dex.v3_fee_manager) — задайте \
         dex.v3_fee_source = {EACH_READ} (решение 281)",
        f.title
    )
}

/// Отвечает ли фабрика на swapFee по ответу вызова (без сети): ответ по ABI uint24 — да; откат или ответ не по ABI — нет.
/// Ошибок нет.
pub fn answers(r: &Result<Bytes, String>) -> bool {
    r.as_ref()
        .is_ok_and(|b| ISpeFactory::swapFeeCall::abi_decode_returns(b).is_ok())
}

/// Пробы для видов кандидатов. Принимает: окружение, виды. Возвращает: пробы и вызовы swapFee(0x0, 0) — только для DEX
/// вида V3 с dex.v3_fee_source ≠ factory_swap_fee, с dex.factory, ответ фабрики которых ещё не известен (ревью 281:
/// раньше — только без настройки). Заранее закрыты DEX с менеджером комиссии без pool_fee_each_read. Ошибок нет.
pub fn plan<'a>(
    ctx: &Ctx,
    fams: impl Iterator<Item = &'a &'static Family>,
) -> (Probes, Vec<(Address, Bytes)>) {
    let mut p = Probes::default();
    let mut calls = Vec::new();
    for f in fams {
        let f: &'static Family = f;
        if p.gates.contains_key(f.id) || !matches!(f.proto, Proto::UniswapV3 | Proto::PancakeV3) {
            continue;
        }
        // Ревью 281 (В3): одно чтение dex.v3_fee_source. Менеджер комиссии (dex.v3_registry_fee_method или
        // dex.v3_fee_manager задана) без pool_fee_each_read — пулы не берутся. Проба пропускается ТОЛЬКО при
        // factory_swap_fee (М5: pool / pool_fee_each_read не обходят защиту 274); нет фабрики — решит проверка пула.
        let source = ctx.dex_text("dex.v3_fee_source", f.id).ok();
        let gate = match (source.as_deref(), ctx.dex_addr("dex.factory", f.id)) {
            (s, _) if s != Some(EACH_READ) && manager_set(ctx, f) => Gate::Shut(manager_reason(f)),
            (Some(FACTORY), _) | (_, Err(_)) => Gate::Open,
            (_, Ok(fa)) => match seen(f.id, fa) {
                Some(true) => Gate::Shut(reason(f, fa)),
                Some(false) => Gate::Open,
                None => {
                    let c = ISpeFactory::swapFeeCall {
                        addr: Address::ZERO,
                        originalFee: U24::ZERO,
                    };
                    calls.push((fa, Bytes::from(c.abi_encode())));
                    p.asked.push((f, fa));
                    Gate::Unknown("нет ответа фабрики на swapFee".into())
                }
            },
        };
        p.gates.insert(f.id, gate);
    }
    (p, calls)
}

impl Probes {
    /// Разбор ответов на вызовы plan (в том же порядке). Ошибок нет: сбой узла — Unknown (не запоминается).
    pub fn settle(&mut self, got: &[Piece]) {
        for (i, (f, fa)) in self.asked.iter().enumerate() {
            let gate = match got.get(i) {
                None => Gate::Unknown("нет ответа фабрики на swapFee".into()),
                Some(Err(node)) => Gate::Unknown(node.clone()),
                Some(Ok(r)) => {
                    let yes = answers(r);
                    remember(f.id, *fa, yes);
                    if yes {
                        Gate::Shut(reason(f, *fa))
                    } else {
                        Gate::Open
                    }
                }
            };
            self.gates.insert(f.id, gate);
        }
    }

    /// Итог для DEX: Ok — пул проверяется дальше, Err — готовый итог (отказ с причиной или «не проверен»). Ошибок нет.
    pub fn check(&self, id: &str) -> Result<(), Verdict> {
        match self.gates.get(id) {
            None | Some(Gate::Open) => Ok(()),
            Some(Gate::Shut(w)) => Err(Verdict::Rejected(w.clone())),
            Some(Gate::Unknown(n)) => Err(Verdict::Failed(n.clone())),
        }
    }
}
