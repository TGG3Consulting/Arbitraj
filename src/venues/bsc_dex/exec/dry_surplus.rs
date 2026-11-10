//! Файл: venues/bsc_dex/exec/dry_surplus.rs
//! Что делает: замер излишка пары, который создало САМО пополнение кошелька в симуляции (решение 252, дополнение
//! «dry_fund», Тигран 10.11: вариант «б»). Факт сервера 10.11 (docs/notes/reflection-facts-2026-11-10.md, раздел 3):
//! у reflection-монеты (SMARS) любой перевод, даже между чужими адресами, поднимает balanceOf пары на долю отражения
//! без Transfer в пару и без sync; пара V2 считает вход как balanceOf − резерв, поэтому продажа после такого перевода
//! получает лишний вход. Пополнение exec/dry_fund.rs — такой перевод: выход продажи симуляции выше настоящего.
//! Здесь — вызовы замера (balanceOf пула и getReserves до пополнения, после него и после обмена) и чистая математика:
//! излишек = balanceOf − резерв, прирост = излишек после − до (настоящий ожидающий излишек, бывший до пополнения, не
//! трогается — его получит и настоящая продажа), выход первого шага V2 на вход симуляции и на вход без прироста.
//! Какие пулы считают вход как balanceOf − резерв (факт кода): V2 — contracts/ArbExecutor.sol _v2Out (L234-L241),
//! Universal Router V2SwapRouter (exec/swap_v2v3.rs, шапка); Solidly — ArbExecutor _solidlyOut (L244-L248); DODO PMM —
//! DPPTrader.sellBase `baseInput = baseBalance.sub(_BASE_RESERVE_)` (DODOEX/contractV2 main,
//! contracts/DODOPrivatePool/impl/DPPTrader.sol L70-L72). Не считают: V3 / Algebra / Elastic / Maverick — вход задан
//! суммой swap и оплачивается в обратном вызове ровно этой суммой (ArbExecutor _step: _expect + swap(amount), _pay);
//! StableSwap — exchange(dx) списывает ровно dx; WooFi — вход = прирост баланса пула вокруг своего перевода (L161-L164).
//! Кто вызывает: exec/dry_sim.rs (вызовы), exec/dry_surplus_cut.rs (математика).
//! От чего зависит: check/{probe_calls, simulate}.rs, pools/abi_v2v3.rs (IV2Pair), math/v2.rs, exec/dry_build.rs.
//! Настройки: нет.

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};

use crate::core::types::{PoolKind, PoolRef};

use super::super::check::probe_calls as pc;
use super::super::check::simulate::{Call, CallResult};
use super::super::math::v2;
use super::super::pools::abi_v2v3::IV2Pair;
use super::dry_build::Ready;

/// Момент замера: до пополнения, сразу после него, после обмена (только резервы V2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Before = 0,
    Funded = 1,
    Swapped = 2,
}

/// Пул первого шага доли, вход которого — пополняемый токен, а вид считает вход как balanceOf − резерв.
#[derive(Debug, Clone)]
pub struct Watch {
    /// Номер доли (Ready.legs).
    pub leg: usize,
    pub pool: Address,
    pub kind: PoolKind,
    /// Пополняемый токен — token0 пула.
    pub zero_in: bool,
    /// Номера вызовов balanceOf(пул): до пополнения, после.
    pub bal: [Option<usize>; 2],
    /// Номера вызовов getReserves (только V2): до пополнения, после, после обмена.
    pub res: [Option<usize>; 3],
}

/// Пулы под замер. Принимает: собранную сделку, пополняемый токен. Возвращает: по одному на долю, у которой первый шаг
/// платит кошелёк этим токеном в пул вида V2 / Solidly / PMM (контракт пула). Ошибок нет.
pub fn watches(r: &Ready, token: Address) -> Vec<Watch> {
    r.legs
        .iter()
        .enumerate()
        .filter_map(|(leg, steps)| {
            let s = steps.first()?;
            let kind = s.entry.pool.kind;
            if !s.from_wallet
                || s.order.token_in != token
                || !matches!(kind, PoolKind::V2 | PoolKind::Solidly | PoolKind::Pmm)
            {
                return None;
            }
            let PoolRef::Contract(pool) = s.entry.pool.reference else {
                return None;
            };
            Some(Watch {
                leg,
                pool,
                kind,
                zero_in: s.entry.pool.token0 == token,
                bal: [None; 2],
                res: [None; 3],
            })
        })
        .collect()
}

/// Вызовы замера в момент `st`: balanceOf(пул) токена (до и после пополнения; после обмена — не читается, токен не
/// нужен) и getReserves пары V2. Только чтение —
/// состояние симуляции не меняют. Принимает: список вызовов, кошелёк (from), токен, пулы, момент. Ошибок нет.
pub fn push(calls: &mut Vec<Call>, wallet: Address, token: Address, ws: &mut [Watch], st: Stage) {
    let i = st as usize;
    for w in ws.iter_mut() {
        if st != Stage::Swapped {
            calls.push(pc::balance(wallet, token, w.pool));
            w.bal[i] = Some(calls.len() - 1);
        }
        if w.kind == PoolKind::V2 {
            calls.push(pc::call(
                wallet,
                w.pool,
                IV2Pair::getReservesCall {}.abi_encode(),
            ));
            w.res[i] = Some(calls.len() - 1);
        }
    }
}

/// Пополнение с замером вокруг него: пулы под замер, вызовы «до», перевод пополнения, вызовы «после».
/// Принимает: список вызовов, (сделка, кошелёк, токен), вызов перевода. Возвращает: пулы замера. Ошибок нет.
pub fn around(
    calls: &mut Vec<Call>,
    (r, wallet, token): (&Ready, Address, Address),
    transfer: Call,
) -> Vec<Watch> {
    let mut ws = watches(r, token);
    push(calls, wallet, token, &mut ws, Stage::Before);
    calls.push(transfer);
    push(calls, wallet, token, &mut ws, Stage::Funded);
    ws
}

/// Резервы пар V2 после обмена (последним чтением симуляции). Принимает: вызовы, кошелёк (from), пулы. Ошибок нет.
pub fn after_swap(calls: &mut Vec<Call>, wallet: Address, ws: &mut [Watch]) {
    push(calls, wallet, Address::ZERO, ws, Stage::Swapped);
}

/// Резервы пары из ответа getReserves: (пополняемого токена, встречного). None — откат или ответ не по ABI.
/// Ошибок нет.
pub fn reserves(r: &CallResult, zero_in: bool) -> Option<(U256, U256)> {
    if !r.ok() {
        return None;
    }
    let x = IV2Pair::getReservesCall::abi_decode_returns(&r.return_data).ok()?;
    let (a, b) = (U256::from(x.reserve0), U256::from(x.reserve1));
    Some(if zero_in { (a, b) } else { (b, a) })
}

/// Прирост излишка (balanceOf − резерв) от пополнения: (баланс после − резерв после) − (баланс до − резерв до), считается
/// как (баланс после + резерв до) − (баланс до + резерв после) — без вычитания «баланс − резерв» по отдельности (у монеты,
/// уменьшающей балансы держателей, баланс бывает ниже резерва). Принимает: (баланс, резерв) до и после; у вида без
/// чтения резерва — резерв 0 (прирост баланса). Не вырос — 0 (обычная монета: перевод чужим адресам пулу ничего не
/// добавляет; ожидающий излишек, бывший до пополнения, в прирост не входит). Ошибок нет.
pub fn grown((b0, r0): (U256, U256), (b1, r1): (U256, U256)) -> U256 {
    b1.saturating_add(r0).saturating_sub(b0.saturating_add(r1))
}

/// Первый шаг V2: выход на вход симуляции и на вход без прироста — getAmountOut пары (math/v2.rs) на резервах той же
/// симуляции перед обменом. Принимает: вход пары в симуляции (рост резерва токена за обмен — пара пишет резерв =
/// баланс, UniswapV2Pair._update), прирост от пополнения, резервы до обмена (токена, встречного), выход пары в
/// симуляции (падение резерва встречного), комиссию пары. Возвращает: (выход в симуляции, выход без прироста).
/// Err — формула на входе симуляции не дала выход симуляции (комиссия или резервы не те, что видел обмен): поправка
/// не была бы точной.
pub fn first_hop(
    input: U256,
    grown: U256,
    r: (U256, U256),
    seen_out: U256,
    fee: (U256, U256),
) -> Result<(U256, U256), String> {
    let full = v2::amount_out(input, r.0, r.1, fee).ok_or_else(|| {
        format!(
            "выход пары на вход {input} не считается (резервы {}, {})",
            r.0, r.1
        )
    })?;
    if full != seen_out {
        return Err(format!(
            "формула пары на входе симуляции {input} даёт {full}, обмен в симуляции отдал {seen_out} — поправка не точна"
        ));
    }
    let net = input
        .checked_sub(grown)
        .filter(|v| !v.is_zero())
        .ok_or_else(|| format!("прирост излишка {grown} не меньше входа пары {input}"))?;
    let cut = v2::amount_out(net, r.0, r.1, fee)
        .ok_or_else(|| format!("выход пары на вход {net} не считается"))?;
    Ok((full, cut))
}
