//! Файл: venues/bsc_dex/pools/verify_algebra_dir.rs
//! Что делает: ревью решения 444 — у вида algebra_dir_fee (Algebra V1.9 с направленными комиссиями, Camelot V3)
//! проверка пула кроме poolByPair фабрики сверяет ВЕРСИЮ КОДА пула: адрес пула = CREATE2(poolDeployer фабрики,
//! keccak256(abi.encode(token0, token1)), хеш кода пула) для одного из хешей настройки dex.pool_code_hashes [DEX].
//! Зачем: в ветке direction-based-fee есть версии со СЖАТЫМ битмапом тиков (TICK_SPACING = 60, коммиты 0d5612b …
//! a3dc950, январь 2023; первая с направленными комиссиями и сырым — 4ada8b0, 06.06.2023), а бот читает СЫРОЙ
//! (load_algebra.rs) — пул другой версии посчитался бы неверно. Хеш не из списка — пул отклоняется с
//! причиной; настройка не задана или пуста — пулы вида не берутся (причина у каждого пула).
//! Кто вызывает: pools/verify_algebra.rs (plan — список хешей и вызов poolDeployer, judge — check).
//! От чего зависит: ctx.rs, abi_algebra_dir.rs (IAlgebraDirFactory), discover.rs (sorted), verify.rs (Candidate,
//! Verdict).
//! Настройки: dex.pool_code_hashes [DEX] (обязательная для algebra_dir_fee).
//!
//! Источник — cryptoalgebra/AlgebraV1.9, ветка direction-based-fee, a45417ed: AlgebraFactory.sol:19 (poolDeployer
//! immutable), :63 (token0 < token1), :127-134 (POOL_INIT_CODE_HASH и computeAddress — тот же CREATE2);
//! AlgebraPoolDeployer.sol:49 (new AlgebraPool{salt: keccak256(abi.encode(token0, token1))}); IAlgebraFactory.sol:65.
//! Хеши версий и их коммиты — tools/arbitrum_record/README.md.

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, keccak256};

use super::super::ctx::Ctx;
use super::abi_algebra_dir::IAlgebraDirFactory;
use super::discover::sorted;
use super::verify::{Candidate, Verdict};

/// Имя настройки допустимых хешей кода пула.
pub const SETTING: &str = "dex.pool_code_hashes";

/// Допустимые хеши кода пула DEX кандидата. Принимает: окружение, кандидата. Возвращает: список хешей.
/// Ошибки (текст причины): настройка не задана, пуста или элемент не 32-байтовый hex.
pub fn hashes(ctx: &Ctx, c: &Candidate) -> Result<Vec<B256>, String> {
    hashes_for(ctx, c.family.id)
}

pub(super) fn hashes_for(ctx: &Ctx, id: &str) -> Result<Vec<B256>, String> {
    let list = ctx
        .dex_list(SETTING, id)
        .map_err(|e| format!("{}: версия кода пула не сверяется", e.message))?;
    if list.is_empty() {
        return Err(format!("{SETTING} [dex:{id}] пуста — пулы вида не берутся"));
    }
    let parse = |h: &String| {
        B256::from_str(h.trim()).map_err(|_| format!("{SETTING}: «{h}» — не хеш (0x + 64 hex)"))
    };
    list.iter().map(parse).collect()
}

/// Вызов poolDeployer() фабрики. Принимает: адрес фабрики. Возвращает: (адрес, данные). Ошибок нет.
pub fn deployer_call(factory: Address) -> (Address, Bytes) {
    (
        factory,
        Bytes::from(IAlgebraDirFactory::poolDeployerCall {}.abi_encode()),
    )
}

/// Адрес пула пары по CREATE2 (AlgebraFactory.sol:133-134, AlgebraPoolDeployer.sol:49). Принимает: деплоер, токены
/// пары в любом порядке, хеш кода пула. Возвращает: адрес. Ошибок нет.
pub fn pool_address(deployer: Address, a: Address, b: Address, hash: B256) -> Address {
    let (t0, t1) = sorted(a, b);
    let mut enc = [0u8; 64];
    enc[12..32].copy_from_slice(t0.as_slice());
    enc[44..64].copy_from_slice(t1.as_slice());
    deployer.create2(keccak256(enc), hash)
}

/// Сверка версии кода пула по ответу poolDeployer(). Принимает: кандидата, адрес пула, ответ узла, хеши.
/// Возвращает: None — пул той версии; Some(Failed) — сбой узла; Some(Rejected) — откат, не по ABI или хеш не из списка.
pub fn check(
    c: &Candidate,
    pool: Address,
    answer: Option<Result<Result<Bytes, String>, String>>,
    hashes: &[B256],
) -> Option<Verdict> {
    let raw = match answer {
        None => return Some(Verdict::Failed("нет ответа poolDeployer() фабрики".into())),
        Some(Err(node)) => return Some(Verdict::Failed(node)),
        Some(Ok(Err(rev))) => {
            return Some(Verdict::Rejected(format!(
                "poolDeployer() фабрики откатился: {rev}"
            )));
        }
        Some(Ok(Ok(b))) => b,
    };
    let deployer = match IAlgebraDirFactory::poolDeployerCall::abi_decode_returns(&raw) {
        Ok(d) => d,
        Err(e) => return Some(Verdict::Rejected(format!("poolDeployer() не по ABI: {e}"))),
    };
    if hashes
        .iter()
        .any(|h| pool_address(deployer, c.coin, c.quote, *h) == pool)
    {
        return None;
    }
    Some(Verdict::Rejected(format!(
        "пул {pool} не из кода версий {SETTING} [dex:{}] (CREATE2 деплоера {deployer}): \
         другая версия Algebra (например, сжатый битмап тиков) — расчёт бота к ней не подходит",
        c.family.id
    )))
}
