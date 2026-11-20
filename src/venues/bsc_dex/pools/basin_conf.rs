//! Файл: venues/bsc_dex/pools/basin_conf.rs
//! Что делает: решение 485 — настройки DEX вида basin_well одним значением: Aquifer (dex.factory), допущенные
//! реализации скважин (dex.basin_well_impls), допущенные функции скважин ConstantProduct2 (dex.basin_cp2) и Stable2
//! (dex.basin_stable2), таблица Stable2 с параметром A (dex.basin_stable2_lut). Скважину может развернуть кто угодно
//! со своими компонентами (README Basin: «Wells may be deployed with malicious components») — бот берёт только
//! скважины официальных реализаций с официальной функцией; насосы (pumps) на обмен не влияют (Well.sol L726-L754:
//! update в try/catch, резервы обмена — свои) — их списка нет.
//! Кто вызывает: pools/{load_basin, verify_basin}.rs.
//! От чего зависит: ctx.rs.
//! Настройки: dex.factory, dex.basin_well_impls, dex.basin_cp2, dex.basin_stable2, dex.basin_stable2_lut [DEX].

use std::str::FromStr;

use alloy_primitives::Address;

use super::super::ctx::Ctx;

/// Какая функция скважины у адреса (по спискам настроек).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    Cp2,
    Stable2,
}

/// Настройки DEX вида basin_well.
#[derive(Debug, Clone, Default)]
pub struct BasinConf {
    /// Aquifer — фабрика и реестр скважин (wellImplementation).
    pub aquifer: Address,
    /// Допущенные реализации скважин (Well, WellUpgradeable).
    pub impls: Vec<Address>,
    /// Допущенные адреса ConstantProduct2.
    pub cp2: Vec<Address>,
    /// Допущенные адреса Stable2.
    pub stable2: Vec<Address>,
    /// Таблица Stable2 (getAParameter); None — скважины Stable2 не берутся.
    pub lut: Option<Address>,
}

impl BasinConf {
    /// Вид функции скважины по адресу. Ошибка (текст) — адреса нет ни в одном списке.
    pub fn kind_of(&self, target: Address) -> Result<FnKind, String> {
        if self.cp2.contains(&target) {
            Ok(FnKind::Cp2)
        } else if self.stable2.contains(&target) {
            Ok(FnKind::Stable2)
        } else {
            Err(format!(
                "функция скважины {target:#x} не из dex.basin_cp2 / dex.basin_stable2 — скважина не берётся"
            ))
        }
    }
}

/// Список адресов настройки (AddressList). Не задан — пусто. Ошибка (текст) — элемент не адрес.
fn list(ctx: &Ctx, name: &str, id: &str) -> Result<Vec<Address>, String> {
    let Ok(raw) = ctx.dex_list(name, id) else {
        return Ok(Vec::new());
    };
    raw.iter()
        .map(|s| Address::from_str(s.trim()).map_err(|_| format!("{name}: «{s}» — не адрес")))
        .collect()
}

/// Настройки DEX. Принимает: окружение, id DEX. Ошибка (текст): не задан dex.factory или dex.basin_well_impls,
/// нет ни одной функции (dex.basin_cp2 и dex.basin_stable2 пусты), элемент списка не адрес.
pub fn read(ctx: &Ctx, id: &str) -> Result<BasinConf, String> {
    let aquifer = ctx.dex_addr("dex.factory", id).map_err(|e| e.message)?;
    let impls = list(ctx, "dex.basin_well_impls", id)?;
    if impls.is_empty() {
        return Err("не задана dex.basin_well_impls — реализации скважин не допущены".into());
    }
    let cp2 = list(ctx, "dex.basin_cp2", id)?;
    let stable2 = list(ctx, "dex.basin_stable2", id)?;
    if cp2.is_empty() && stable2.is_empty() {
        return Err("не заданы dex.basin_cp2 и dex.basin_stable2 — функций скважин нет".into());
    }
    let lut = ctx.dex_addr("dex.basin_stable2_lut", id).ok();
    Ok(BasinConf {
        aquifer,
        impls,
        cp2,
        stable2,
        lut,
    })
}
