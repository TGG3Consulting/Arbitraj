//! Файл: venues/bsc_dex/hanji/live.rs.
//! Что делает: связывает публичный рынок Hanji с исполнением расчёта по его стакану.
//! Кто вызывает: ручная проверка котировки через шлюз.
//! От чего зависит: официальный список рынков, Base RPC, ABI getConfig и FastQuoterHelper.
//! Настройки: используются HTTP-лимиты и узел выбранной DEX-площадки.
//! Сверка цены из интерфейса. Запрашивается официальный реестр, затем токены,
//! масштабы лота, комиссия и уровни одной стороны книги на одном хеше Base.
use super::{
    Level, Side, Units,
    model::{Preview, Request},
};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    http,
};
use crate::venues::bsc_dex::{ctx::Ctx, node::rpc};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U64, U256, address};
use serde::Deserialize;
use serde_json::json;

const REGISTRY: &str = "https://api-base.hanji.io/aggregators/markets";
const HELPER: Address = address!("c032b6dfec3511a00cde9ea341d140f4733609de");
const DEPTH: u32 = 64;

sol! {
    interface IHanji {
        function getConfig() external view returns (
            uint256 scaleX, uint256 scaleY, address x, address y, bool supportsEth, bool xIsWrapped,
            address askTrie, address bidTrie, uint64 adminFee, uint64 aggressiveFee, uint64 passiveFee,
            uint64 passivePayout, bool invokeOnTrade
        );
        function assembleOrderbookFromOrders(address market, bool asks, uint24 maxLevels) external view
            returns (uint72[] prices, uint128[] shares);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Market {
    orderbook_address: Address,
    base_token: Token,
    quote_token: Token,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Token {
    contract_address: Address,
}
#[derive(Deserialize)]
struct Header {
    number: U64,
    hash: B256,
}

async fn canonical<C: SolCall>(
    ctx: &Ctx,
    to: Address,
    call: C,
    hash: B256,
) -> GatewayResult<C::Return> {
    let data = Bytes::from(call.abi_encode());
    let raw: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":to, "data":data}, {"blockHash":hash,"requireCanonical":true}]),
    )
    .await?;
    C::abi_decode_returns(&raw).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            format!("Hanji: неверный результат {}", C::SIGNATURE),
        )
    })
}

pub(in crate::venues::bsc_dex) async fn preview(ctx: &Ctx, req: Request) -> GatewayResult<Preview> {
    if ctx.chain_id() != Some(8453) {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Hanji: поддерживаемый реестр относится к Base (8453)",
        ));
    }
    let amount = U256::from_str_radix(&req.amount_raw, 10)
        .ok()
        .filter(|v| !v.is_zero())
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NotApplicable,
                "Нужна положительная целая сумма в минимальных единицах токена",
            )
        })?;
    if req.input == req.output || req.input.is_zero() || req.output.is_zero() {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Для Hanji укажите два разных контракта. Для ETH используйте WETH",
        ));
    }
    let (status, body) = http::exchange(ctx.http.get(REGISTRY), ctx.limits()?)
        .await
        .map_err(|_| {
            ctx.err(
                ErrorCode::Unreachable,
                "Hanji: не получен публичный реестр рынков",
            )
        })?;
    if !status.is_success() {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!("Hanji: реестр вернул HTTP {status}"),
        ));
    }
    let markets: Vec<Market> = serde_json::from_str(&body).map_err(|_| {
        ctx.err(
            ErrorCode::BadResponse,
            "Hanji: неверный ответ реестра рынков",
        )
    })?;
    let mut matches = markets.iter().filter(|m| {
        let x = m.base_token.contract_address;
        let y = m.quote_token.contract_address;
        req.market.is_none_or(|id| m.orderbook_address == id)
            && ((x == req.input && y == req.output) || (x == req.output && y == req.input))
    });
    let market = matches.next().ok_or_else(|| {
        ctx.err(
            ErrorCode::NoRoute,
            "Hanji: такой пары контрактов нет в реестре",
        )
    })?;
    if matches.next().is_some() {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "В реестре несколько рынков этой пары. Укажите адрес рынка в поле market",
        ));
    }
    let chain: U64 = rpc::call(ctx, "eth_chainId", json!([])).await?;
    if chain.to::<u64>() != 8453 {
        return Err(ctx.err(
            ErrorCode::NotApplicable,
            "Узел Hanji подключён к другой сети",
        ));
    }
    let head: Header = rpc::call(ctx, "eth_getBlockByNumber", json!(["latest", false])).await?;
    let buy = req.output == market.base_token.contract_address;
    let (cfg, book) = tokio::try_join!(
        canonical(
            ctx,
            market.orderbook_address,
            IHanji::getConfigCall {},
            head.hash
        ),
        canonical(
            ctx,
            HELPER,
            IHanji::assembleOrderbookFromOrdersCall {
                market: market.orderbook_address,
                asks: buy,
                maxLevels: DEPTH.try_into().expect("64 fits uint24")
            },
            head.hash
        )
    )?;
    if cfg.x != market.base_token.contract_address
        || cfg.y != market.quote_token.contract_address
        || book.prices.len() != book.shares.len()
        || book.prices.len() > DEPTH as usize
    {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "Адреса рынка или форма стакана Hanji расходятся с источником",
        ));
    }
    let levels: Vec<_> = book
        .prices
        .iter()
        .zip(book.shares)
        .map(|(price, shares)| Level {
            price: price.to::<u128>(),
            shares,
        })
        .collect();
    let units = Units {
        x: cfg.scaleX,
        y: cfg.scaleY,
        fee_rate: u128::from(cfg.aggressiveFee) + u128::from(cfg.passivePayout),
    };
    let fill = units
        .fill(&levels, if buy { Side::BuyX } else { Side::SellX }, amount)
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                "Hanji: на доступных уровнях нет исполнимого лота этого размера",
            )
        })?;
    Ok(Preview {
        execution_checked: false,
        market: market.orderbook_address,
        block: head.number.to::<u64>(),
        hash: head.hash,
        input: req.input,
        output: req.output,
        spent_raw: fill.spent.to_string(),
        received_raw: fill.received.to_string(),
        unspent_raw: fill.unspent.to_string(),
        fee_asset: cfg.y,
        fee_raw: fill.fee_y.to_string(),
        last_price: fill.last_price.to_string(),
    })
}
