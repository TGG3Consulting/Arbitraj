//! Файл: core/types/pool_vault_v2.rs
//! Что делает: решение 473 (T450-03…07) — кривые пулов хранилища Balancer V2, которых нет у весового и стейблсвап-пула
//! решений 289 / 450: стейблсвап с множителями FixedPoint, в которые входит курс токена (composable stable v1–v6,
//! meta-stable, stable phantom), управляемый весовой пул (managed v1 / v2: комиссия от суммы с множителем, пределы цены
//! BPT — circuit breakers) и линейный пул (aave / erc4626 / yearn / unbutton: главный и обёрнутый токен, цели). Без
//! названия площадки: только числа, которые пул отдаёт на блоке чтения. Вынесено из pool_vault.rs (предел размера).
//! Кто вызывает: core/types/pool_vault.rs (VaultCurve::V2), адаптер DEX (pools/load_balancer_more*.rs создаёт,
//! math/balancer_v2_more.rs, math/balancer_linear.rs считают).
//! От чего зависит: alloy-primitives, serde.
//! Настройки: нет.

use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

/// Кривая пула Balancer V2 решения 473. Порядок векторов — порядок токенов пула в хранилище (с BPT, если он среди них).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum V2Curve {
    /// Стейблсвап с курсами: A × точность A на блоке чтения, множитель FixedPoint каждого токена (1e18·10^(18 − decimals),
    /// умноженный mulDown на курс — тот, что пул возьмёт при обмене на блоке чтения), индекс BPT пула среди токенов
    /// (composable stable, stable phantom; у meta-stable — None) и версия StableMath (v2 — инвариант по формуле Curve с
    /// округлением вниз, composable stable; иначе — v1 с округлением вверх, meta-stable и stable phantom).
    ///
    /// `alt` — множители, если кеш курса истечёт к блоку исполнения (кеш истекает при block.timestamp > expires; время
    /// блока — целые секунды, следующий блок может быть в следующей секунде): у токенов с expires < время чтения + 1 —
    /// курс поставщика. None — ни у одного токена кеш к следующей секунде не истекает. Обмен считается по худшему из
    /// двух наборов (math/balancer_v2_more.rs).
    StableRated {
        amp: U256,
        factors: Vec<U256>,
        bpt: Option<u8>,
        v2: bool,
        alt: Option<Vec<U256>>,
    },
    /// Управляемый весовой пул (managed v1 / v2): веса на блоке чтения (у BPT — 0), нижняя и верхняя граница цены BPT
    /// каждого токена в единицах хранения пула (0 — границы нет), фактический выпуск BPT (getActualSupply) и индекс BPT.
    Managed {
        weights: Vec<U256>,
        lower: Vec<U256>,
        upper: Vec<U256>,
        supply: U256,
        bpt: u8,
    },
    /// Линейный пул: индексы главного, обёрнутого токена и BPT, множители FixedPoint (у обёрнутого — с курсом
    /// getWrappedTokenRate), нижняя и верхняя цель баланса главного токена (18 знаков) и выпуск BPT, от которого пул
    /// отнимает баланс BPT в хранилище (виртуальный выпуск).
    Linear {
        main: u8,
        wrapped: u8,
        bpt: u8,
        factors: Vec<U256>,
        lower: U256,
        upper: U256,
        supply: U256,
    },
}
