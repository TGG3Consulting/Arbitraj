//! Файл: venues/bsc_dex/quote/emptiness.rs
//! Что делает: пустой ли пул — по его локальному состоянию: V2 — нулевой резерв, V3 и CL — ликвидность на текущем
//! тике 0 (liquidity() / getLiquidity) И в прочитанной карте нет ни одного установленного тика (решение 393), Bin — в прочитанной полосе нет непустых бинов. Пустой пул — не «сбой
//! квотера»: квотер на нём откатывается (QuoterV2 — Error("Unexpected")) или считает по брошенной ликвидности далеко
//! от рынка. Такой пул не котируется и не сверяется с квотером, пока событие (Mint / ModifyLiquidity / Sync) не вернёт
//! ему ликвидность. Там же — причины исключения для сверки с квотером (check/quoter_cycle.rs, exec/precheck.rs).
//! Кто вызывает: quote/select.rs, quote/quoter.rs, exec/precheck.rs, pools/listen_log.rs (пул снова не пуст).
//! От чего зависит: pools/registry.rs, core/types (PoolState).
//! Настроек нет.
//!
//! Состояние пулов — research/flow/bsc_dex.md, раздел 3 (V2 getReserves/Sync, V3 slot0/liquidity), bsc_v4.md, раздел 4
//! (getSlot0, getLiquidity, getBin). Откат QuoterV2 на пуле с liquidity() = 0 проверен координатором eth_call к BSC
//! (PCS V3 ADA/USDT fee 100/500/10000 и др.; Issue #7).

use crate::core::types::PoolState;
use alloy_primitives::U256;

use super::super::pools::registry::Entry;

/// Причина: пул пуст.
pub const EMPTY: &str = "пустой пул (ликвидность 0)";

/// Причина: у пула не хватает ликвидности на сумму сверки — один класс для локального расчёта и квотера.
pub const SHORT: &str = "у пула не хватает ликвидности на сумму сверки";

/// Уточнение: локальный расчёт не прошёл всю сумму в прочитанной карте тиков (бинов).
pub const SHORT_LOCAL: &str = "расчёт вышел за прочитанную карту тиков (бинов)";

/// Причина «не хватает ликвидности» с уточнением. Ошибок нет.
pub fn short(detail: &str) -> String {
    format!("{SHORT} ({detail})")
}

/// Есть ли в прочитанной карте тиков пула установленный тик (ликвидность рядом с ценой). Нет карты — false. Ошибок нет.
pub fn has_ticks(e: &Entry) -> bool {
    e.ticks
        .as_ref()
        .is_some_and(|m| m.ticks.values().any(|t| t.gross > 0))
}

/// Пуст ли пул (см. шапку). Bin без прочитанных бинов — не пуст (не прочитан — это решает достоверность). Ошибок нет.
pub fn empty(e: &Entry) -> bool {
    match &e.pool.state {
        PoolState::Surge(s) => s.liquidity.is_zero() || s.stock.is_zero() || s.invariant.is_zero(),
        PoolState::Dagrow(s) => s.reserves.contains(&U256::ZERO) || s.fees.iter().all(Option::is_none),
        PoolState::UmbraeDamm(s) => s.reserves.contains(&U256::ZERO),
        PoolState::Bundles(s) => s.market.supply<=U256::from(1000) || s.market.reserves.contains(&U256::ZERO),
        PoolState::Antfarm(s) => s.market.lp_supply<=U256::from(1000) || s.market.reserves.contains(&U256::ZERO),
        PoolState::Temple(s) => s.reserves.contains(&U256::ZERO) || ((!s.amm_authorized || (e.coin!=s.stable && e.usdt!=s.stable))
            && (s.defense_balance.is_zero() || s.ratio.is_none_or(|r|r.contains(&U256::ZERO)))),
        PoolState::BancorV3(s)=>s.markets.iter().flatten().any(|m|!m.enabled || m.base.is_zero() || m.bnt.is_zero()) || s.vault_balances.iter().all(U256::is_zero),
        PoolState::UniswapV1(s)=>s.markets.iter().flatten().any(|e|e.erc20.is_zero()||e.eth.is_zero()),
        PoolState::Rubicon(s) => s.offers.iter().all(Vec::is_empty),
        PoolState::Carbon(s) => s.strategies.iter().flat_map(|st|&st.orders).all(|o|o.y==0 || (o.a==0 && o.b==0)),
        PoolState::MintClub(s) => s.steps.last().is_none_or(|&(end,_)| alloy_primitives::U256::from(end)<=s.supply && (s.bonded.is_zero() || s.available.is_zero())),
        PoolState::Memebox(s) => s.reserves.iter().any(|r|r.is_zero()),
        PoolState::CandySwap(s) => s.reserves.iter().any(|r|r.is_zero()),
        PoolState::Virtuals(s) => s.invariant.is_zero() || s.backing.iter().all(|v|v.is_zero()),
        PoolState::EtherVista(s) => s.reserve0.is_zero() || s.reserve1.is_zero(),
        PoolState::ToonSwap(s) => s.reserves.contains(&U256::ZERO),
        PoolState::V2 { reserve0, reserve1 } | PoolState::Oasis { reserve0, reserve1, .. } | PoolState::DeltaSwap { reserve0, reserve1, .. } => reserve0.is_zero() || reserve1.is_zero(),
        // Решение 393 (t178): ликвидность 0 на ТЕКУЩЕМ тике — ещё не пустой пул, если в прочитанной карте есть
        // установленный тик: цена в «дыре» между позициями, обмен в сторону позиции проходит дыру даром (шаг обмена
        // при L = 0 — вход 0, цена до следующего тика: math/swap_step.rs, как SwapMath.computeSwapStep). Какая сторона
        // реально проходит — решает расчёт стороны (quote/side_fit.rs) и сверка с квотером по стороне.
        PoolState::V3 { liquidity, .. }
        | PoolState::Cl { liquidity, .. }
        // Algebra (THENA Fusion, решения 80, 81): одна ликвидность (reinvestL нет) — пуст при liquidity == 0.
        | PoolState::Algebra { liquidity, .. } => *liquidity == 0 && !has_ticks(e),
        // Elastic: ликвидность обмена L = base_l + reinvest_l — пуст, когда оба нуля (решение 81).
        PoolState::Elastic {
            base_l, reinvest_l, ..
        } => *base_l == 0 && *reinvest_l == 0,
        PoolState::Bin { .. } => e.bins.as_ref().is_some_and(|b| b.reserves.is_empty()),
        // DODO PMM (решение 86b): пуст, когда оба резерва нули (одной стороны достаточно для своей стороны обмена —
        // односторонний DVM с Q=0 продаёт котируемую за базовую; стороны фильтрует глубина, решение 73).
        PoolState::Pmm(d) => d.b.is_zero() && d.q.is_zero(),
        // Solidly (Topaz v2, THENA V1; решения 80, 85): пуст при нулевом любом резерве (как V2 — обмен невозможен).
        // Решение 273: фабрика на паузе — пуст (обмен откатился бы), как StableSwap halted.
        PoolState::Solidly {
            reserve0,
            reserve1,
            gate,
            ..
        } => gate.paused || reserve0.is_zero() || reserve1.is_zero(),
        // StableSwap (решение 122 F): пуст, если нулевой баланс любой монеты пары (обмен по паре невозможен).
        // Решение 122 F, 3: пул на паузе или выключен (kill) — пуст (обмен откатился бы).
        PoolState::StableSwap(d) => d.halted || [d.i0, d.i1]
            .iter()
            .any(|i| d.balances.get(*i as usize).is_none_or(|b| b.is_zero())),
        // PMM от оракула (WooFi, решение 122 F, 3): пуст, когда контракт на паузе или оракул не признаёт цену.
        PoolState::OraclePmm(d) if super::uses_wallet_quoter(e.family.variant) || e.family.proto == super::super::pools::families::Proto::Elfomo => {
            d.paused || !d.feasible || (d.ladder_quote_in.is_empty() && d.ladder_base_in.is_empty())
        }
        PoolState::OraclePmm(d) => d.paused || !d.feasible || d.price.is_zero(),
        // TickBin (Maverick V2, решение 122 F): пуст при нулевом любом резерве пула (обмен в одну из сторон невозможен).
        PoolState::TickBin(d) => d.reserve0.is_zero() || d.reserve1.is_zero(),
        // V2 с усилением (решение 287): пуст при нулевом реальном резерве (выход должен быть меньше реального резерва).
        PoolState::AmpV2(d) => d.reserve0.is_zero() || d.reserve1.is_zero(),
        // Пара с фиктивными резервами (решение 288): пуста при нулевом резерве или фиктивном резерве (getAmountOut откатит).
        PoolState::Fictive(d) => [d.reserve0, d.reserve1, d.fictive0, d.fictive1].iter().any(|x| x.is_zero()),
        // Пул в общем хранилище (решение 289): пуст на паузе или при нулевом балансе любого токена пары.
        PoolState::VaultPool(d) => d.paused || [d.i0, d.i1]
            .iter()
            .any(|i| d.balances.get(*i as usize).is_none_or(|b| b.is_zero())),
        // Crypto-пул Curve (решения 442, 453): пуст, когда обмены остановлены (halted) или баланс монеты пары нулевой.
        PoolState::Crypto(d) => d.halted || [d.i0, d.i1]
            .iter()
            .any(|i| d.balances.get(*i as usize).is_none_or(|b| b.is_zero())),
        // Бины с изменчивой комиссией (решение 290): пуст, когда в прочитанной полосе нет непустых бинов (как Bin).
        PoolState::LbBin(_) => e.bins.as_ref().is_some_and(|b| b.reserves.is_empty()),
        // Пул Fluid DEX T1 (решение 440): пауза, части выключены или нулевые мнимые резервы (quote/fluid_depth.rs).
        PoolState::SmartColDebt(d) => super::fluid_depth::empty(d),
        // Пул Shell Protocol Proteus (решение 486): заморожен или баланс стороны меньше MIN_BALANCE (shell_depth.rs).
        PoolState::Proteus(d) => super::shell_depth::empty(d),
        // Ekubo (решение 447): ликвидность 0 и в прочитанной карте нет тиков — обмен не даст выхода (как V3, решение 393).
        PoolState::Ekubo(d) => d.liquidity == 0 && d.ticks.is_empty(),
        // Скважина Basin (решение 485): нулевой резерв или нулевое число LP-токенов (quote/basin_depth.rs).
        PoolState::BasinWell(d) => super::basin_depth::empty(d),
    }
}
