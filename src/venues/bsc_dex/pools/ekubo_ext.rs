//! Файл: venues/bsc_dex/pools/ekubo_ext.rs
//! Что делает: расширение пула Ekubo (решение 454) — какое официальное расширение стоит в config ключа и как оно меняет
//! обмен (разбор по исходнику — docs/DECISIONS.md, решение 454):
//! - MEVCapture — beforeSwap откатывает обмен не через forward (обмен — Core.forward), доплата с выхода
//!   (math/ekubo_mev.rs; решение 447);
//! - TWAMM — beforeSwap исполняет виртуальные ордера до обмена (меняет цену пула): котировка сначала проводит их
//!   (math/ekubo_virtual.rs), состояние — pools/load_ekubo_twamm.rs;
//! - Oracle — beforeSwap только пишет снимок цены в своё хранилище (обмен не меняет) — допуск без своей математики;
//! - BoostedFees (оба варианта) — beforeSwap (концентрированный) только начисляет пожертвованные комиссии
//!   (Core.accumulateAsFees меняет комиссии на ликвидность, не цену, тик и ликвидность) — допуск без своей математики.
//!
//! Других расширений (на Arbitrum развёрнуты только эти — broadcast DeployAll.s.sol/42161) нет: пул с неизвестным
//! расширением не берётся. Расширение, адрес которого не задан в настройках, — тоже (причина на экране).
//! Кто вызывает: pools/ekubo_accept.rs, pools/load_ekubo.rs, exec/executor/entry.rs (forward только MEVCapture).
//! От чего зависит: ctx.rs.
//! Настройки: dex.ekubo_mev_capture, dex.ekubo_twamm, dex.ekubo_oracle, dex.ekubo_boosted_fees [DEX].
//!
//! Источник — EkuboProtocol/evm-contracts: @61d09daa src/extensions/MEVCapture.sol L84-L86 (beforeSwap — откат),
//! src/extensions/TWAMM.sol L43-L54 (call points), L655-L657 (beforeSwap → lockAndExecuteVirtualOrders), L394-L600
//! (исполнение виртуальных ордеров — CORE.swap), src/extensions/Oracle.sol L27-L38, L193-L201 (beforeSwap →
//! maybeInsertSnapshot — только своё хранилище, L97-L141), src/Core.sol L228-L276 (accumulateAsFees);
//! @69b642d src/extensions/BoostedFees.sol L28-L39 (call points), L100-L102 (beforeSwap → maybeAccumulateFees),
//! L138-L156 (locked: accumulateAsFees + updateSavedBalances), L158-L237 (maybeAccumulateFees); TWAMM @8c1b721 — тот же код исполнения ордеров
//! (git diff 61d09da..8c1b721 src/extensions/TWAMM.sol — только оплата при изменении ордера).

use std::str::FromStr;

use alloy_primitives::Address;

use super::super::ctx::Ctx;

/// Расширение пула.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext {
    /// Без расширения.
    None,
    /// MEVCapture (dex.ekubo_mev_capture).
    MevCapture,
    /// TWAMM (dex.ekubo_twamm).
    Twamm,
    /// Oracle (dex.ekubo_oracle) — обмен не меняет.
    Oracle,
    /// BoostedFees (dex.ekubo_boosted_fees) — обмен не меняет.
    BoostedFees,
}

/// Адрес в списке адресов настройки (AddressList) DEX. Не задана — false. Ошибок нет.
fn listed(ctx: &Ctx, name: &str, dex: &str, a: Address) -> bool {
    ctx.dex_list(name, dex).is_ok_and(|l| {
        l.iter()
            .filter_map(|s| Address::from_str(s.trim()).ok())
            .any(|x| x == a)
    })
}

/// Адреса TWAMM DEX (dex.ekubo_twamm; не задана — пусто). Ошибок нет.
pub fn twamm_list(ctx: &Ctx, dex: &str) -> Vec<Address> {
    ctx.dex_list("dex.ekubo_twamm", dex)
        .map(|l| {
            l.iter()
                .filter_map(|s| Address::from_str(s.trim()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Вид расширения по адресу из config. Принимает: окружение, DEX, адрес. Ошибки (текст): расширение не из настроек.
pub fn classify(ctx: &Ctx, dex: &str, ext: Address) -> Result<Ext, String> {
    if ext == Address::ZERO {
        return Ok(Ext::None);
    }
    if ctx.dex_addr("dex.ekubo_mev_capture", dex).ok() == Some(ext) {
        return Ok(Ext::MevCapture);
    }
    if listed(ctx, "dex.ekubo_twamm", dex, ext) {
        return Ok(Ext::Twamm);
    }
    if ctx.dex_addr("dex.ekubo_oracle", dex).ok() == Some(ext) {
        return Ok(Ext::Oracle);
    }
    if listed(ctx, "dex.ekubo_boosted_fees", dex, ext) {
        return Ok(Ext::BoostedFees);
    }
    Err(format!(
        "расширение пула {ext:#x} не задано ни в одной настройке расширений Ekubo \
         (dex.ekubo_mev_capture, dex.ekubo_twamm, dex.ekubo_oracle, dex.ekubo_boosted_fees) — \
         пул не берётся"
    ))
}
