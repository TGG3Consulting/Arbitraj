//! Файл: venues/bsc_dex/check/probe.rs
//! Что делает: пробный обмен в пуле V2/V3 через eth_simulateV1 (решение 30) — налог на покупку, продажу и перевод
//! и газ обмена в обе стороны. В одном симулированном блоке, от имени кошелька бота (без подписи):
//! USDT на кошелёк из другого пула → approve роутеру → расчёт без налога (роутер getAmountsOut / QuoterV2) →
//! покупка через роутер → сколько монет пришло (налог покупки) → перевод половины в пул (налог продажи: насколько
//! вырос баланс пула) → перевод четверти на пустой адрес (налог перевода) → продажа остатка через роутер (газ продажи).
//! Кто вызывает: check/taxes.rs.
//! От чего зависит: check/simulate.rs, check/probe_calls.rs, exec/calls.rs, pools/registry.rs.
//! Настройки: dex.router, dex.quoter [вид пула V3]; срок — передаёт вызывающий.
//!
//! Официального рецепта измерения налога нет (research/flow/bsc_dex.md, раздел 6); механика — из исходников:
//! FOT-функция V2 считает вход пары как balanceOf(pair) − reserve (UniswapV2Router02.sol#L321-L355), V3 не поддерживает
//! FOT-токены (swap-faq.md#L20-L27). Получатель налога перевода — адрес без кода и истории: последние 20 байт
//! keccak256(адрес кошелька) — вычисляется, не зашит.

use alloy_primitives::{Address, U256, keccak256};

use crate::core::types::PoolRef;
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::exec::calls::{self, Swap};
use super::super::pools::families::Proto;
use super::super::pools::registry::Entry;
use super::probe_calls as pc;
use super::simulate::{self, Call};

/// Что измерено (в минимальных единицах; проценты считает taxes.rs).
#[derive(Debug, Clone)]
pub struct Measured {
    /// Ожидаемый выход покупки без налога и сколько пришло на самом деле.
    pub buy_expected: U256,
    pub buy_got: U256,
    /// Отправлено в пул и сколько пул получил.
    pub sell_sent: U256,
    pub sell_got: U256,
    /// Отправлено на пустой адрес и сколько он получил.
    pub transfer_sent: U256,
    pub transfer_got: U256,
    pub gas_buy: u64,
    /// Газ продажи; None — продажа откатилась (причина в журнале у вызывающего).
    pub gas_sell: Option<u64>,
}

/// Пробный обмен. Принимает: пул (V2/V3), источник USDT (другой пул), кошелёк, сумму USDT, роутер, квотер (V3), срок.
/// Возвращает: Ok(Measured) или Ok(Err(причина)) — проба не прошла; Err — узел (сеть, метод не поддерживается).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    ctx: &Ctx,
    e: &Entry,
    source: Address,
    wallet: Address,
    usdt_amount: U256,
    router: Address,
    quoter: Option<Address>,
    deadline: U256,
) -> GatewayResult<Result<Measured, String>> {
    let PoolRef::Contract(pool) = e.pool.reference else {
        return Ok(Err("не пул V2/V3".into()));
    };
    let (usdt, coin) = (e.usdt, e.coin);
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    let swap = |token_in, token_out, amount_in| Swap {
        token_in,
        token_out,
        amount_in,
        min_out: U256::ZERO,
        recipient: wallet,
        deadline,
        fee_on_transfer: e.family.proto == Proto::V2,
    };
    let Some(expected) = pc::expected(e, wallet, router, quoter, usdt_amount) else {
        return Ok(Err("нет квотера для расчёта без налога".into()));
    };
    let buy = calls::direct(e, &swap(usdt, coin, usdt_amount))?;
    let mut list = vec![
        pc::transfer(source, usdt, wallet, usdt_amount),
        pc::approve(wallet, usdt, router, usdt_amount),
        expected,
        Call {
            from: wallet,
            to: router,
            input: buy,
        },
        pc::balance(wallet, coin, wallet),
    ];
    let first = simulate::run(ctx, &list).await?;
    if let Some((i, r)) = first.iter().enumerate().find(|(_, r)| !r.ok()) {
        return Ok(Err(format!(
            "шаг {i} пробной покупки откатился: {}",
            r.why()
        )));
    }
    let (Some(buy_expected), Some(bought)) = (pc::expected_out(e, &first[2]), pc::u256(&first[4]))
    else {
        return Ok(Err("ответ расчёта или баланса не по ABI".into()));
    };
    let (half, quarter) = (bought / U256::from(2u8), bought / U256::from(4u8));
    let rest = bought - half - quarter;
    list.extend([
        pc::balance(wallet, coin, pool),
        pc::transfer(wallet, coin, pool, half),
        pc::balance(wallet, coin, pool),
        pc::transfer(wallet, coin, empty, quarter),
        pc::balance(wallet, coin, empty),
        pc::approve(wallet, coin, router, rest),
        Call {
            from: wallet,
            to: router,
            input: calls::direct(e, &swap(coin, usdt, rest))?,
        },
    ]);
    let all = simulate::run(ctx, &list).await?;
    if let Some((i, r)) = all.iter().enumerate().take(11).find(|(_, r)| !r.ok()) {
        return Ok(Err(format!(
            "шаг {i} пробы (перевод монеты) откатился: {}",
            r.why()
        )));
    }
    let (Some(pb0), Some(pb1), Some(xb)) =
        (pc::u256(&all[5]), pc::u256(&all[7]), pc::u256(&all[9]))
    else {
        return Ok(Err("ответ баланса не по ABI".into()));
    };
    Ok(Ok(Measured {
        buy_expected,
        buy_got: bought,
        sell_sent: half,
        sell_got: pb1.saturating_sub(pb0),
        transfer_sent: quarter,
        transfer_got: xb,
        gas_buy: all[3].gas_used.to(),
        gas_sell: all[11].ok().then(|| all[11].gas_used.to()),
    }))
}
