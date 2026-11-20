//! Файл: venues/bsc_dex/pools/basin_conf.rs
//! Что делает: решение 485 — настройки DEX вида basin_well одним значением: Aquifer (dex.factory), допущенные
//! реализации скважин (dex.basin_well_impls), допущенные функции скважин ConstantProduct2 (dex.basin_cp2) и Stable2
//! (dex.basin_stable2) с таблицами A (dex.basin_stable2_lut — список, k-я таблица — для k-й функции Stable2), и правило
//! «реализация скважины допущена» (impl_ok). Скважину может развернуть кто угодно со своими компонентами (README Basin:
//! «Wells may be deployed with malicious components») — бот берёт только скважины официальных реализаций с официальной
//! функцией; насосы (pumps) на обмен не влияют (Well.sol L726-L754: update в try/catch, резервы обмена — свои) — их
//! списка нет.
//! Кто вызывает: pools/{load_basin, verify_basin, verify_basin_judge}.rs.
//! От чего зависит: ctx.rs.
//! Настройки: dex.factory, dex.basin_well_impls, dex.basin_cp2, dex.basin_stable2, dex.basin_stable2_lut [DEX].

use std::str::FromStr;

use alloy_primitives::{Address, B256, U256, keccak256};

use super::super::ctx::Ctx;

/// Какая функция скважины у адреса (по спискам настроек): Stable2 — с номером в dex.basin_stable2 (он же номер таблицы).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnKind {
    Cp2,
    Stable2(usize),
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
    /// Таблицы Stable2 (getAParameter): k-я — для stable2[k]. Пусто — скважины Stable2 не берутся.
    pub luts: Vec<Address>,
}

impl BasinConf {
    /// Вид функции скважины по адресу. Ошибка (текст) — адреса нет ни в одном списке или у Stable2 нет таблицы.
    pub fn kind_of(&self, target: Address) -> Result<FnKind, String> {
        if self.cp2.contains(&target) {
            return Ok(FnKind::Cp2);
        }
        match self.stable2.iter().position(|a| *a == target) {
            Some(k) if k < self.luts.len() => Ok(FnKind::Stable2(k)),
            Some(k) => Err(format!(
                "Stable2 {target:#x} — {}-я в dex.basin_stable2, а таблицы A с этим номером в \
                 dex.basin_stable2_lut нет",
                k + 1
            )),
            None => Err(format!(
                "функция скважины {target:#x} не из dex.basin_cp2 / dex.basin_stable2 — скважина не \
                 берётся"
            )),
        }
    }
}

/// Слот реализации прокси ERC-1967: bytes32(uint256(keccak256("eip1967.proxy.implementation")) − 1) (EIP-1967; так же
/// OpenZeppelin ERC1967Upgrade _IMPLEMENTATION_SLOT = 0x3608…2bbc). Ошибок нет.
pub fn impl_slot() -> B256 {
    let h = U256::from_be_bytes(keccak256(b"eip1967.proxy.implementation").0);
    B256::from((h - U256::from(1u8)).to_be_bytes::<32>())
}

/// Реализация скважины допущена. Принимает: настройки, скважину, wellImplementation(скважина) Aquifer'а, реализацию из
/// слота ERC-1967 скважины (eth_getStorageAt — не ответ контракта; ноль — не прокси) и wellImplementation этой
/// реализации. Возвращает: Ok — клон Aquifer'а с реализацией из списка или прокси на такой клон; Err — причина.
pub fn impl_ok(
    conf: &BasinConf,
    well: Address,
    registered: Address,
    slot_impl: Address,
    slot_registered: Address,
) -> Result<(), String> {
    if conf.impls.contains(&registered) {
        return Ok(());
    }
    if slot_impl.is_zero() {
        return Err(format!(
            "скважина {well:#x}: реализация в Aquifer {registered:#x} не из dex.basin_well_impls, \
             слот ERC-1967 пуст"
        ));
    }
    if conf.impls.contains(&slot_registered) {
        return Ok(());
    }
    Err(format!(
        "скважина {well:#x} — прокси на {slot_impl:#x}: его реализация в Aquifer \
         {slot_registered:#x} не из dex.basin_well_impls"
    ))
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
    let luts = list(ctx, "dex.basin_stable2_lut", id)?;
    Ok(BasinConf {
        aquifer,
        impls,
        cp2,
        stable2,
        luts,
    })
}
