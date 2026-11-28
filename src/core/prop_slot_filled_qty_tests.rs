//! О-56. Из архивной симуляции Taurus берём покупку. Учёт следующего вывода и продажи проверяет
//! тестовый получатель без комиссии. Самостоятельная обратная симуляция проверяется отдельно.

use alloy_primitives::{Address, U256, address};
use crate::{
    core::{deal::Deal, fills, remain_fact, types::{
        Direction, Side, TradeRequest, TradeResult, Transfer,
        TransferStatus, VenueId,
    }},
    venues::bsc_dex::{exec::{fill, settlement_swap}, send::receipt::Receipt},
};
use crate::gateway::settlement_intent::SpotFill;
use flate2::read::GzDecoder;
use rust_decimal::Decimal;
use serde_json::{Value, json};

fn file(raw: &[u8]) -> Value {
    serde_json::from_reader(GzDecoder::new(raw)).unwrap()
}

/// Декодирует журнал независимой симуляции. У кадра нет уплаченной сетевой комиссии.
fn receipt(row: &Value) -> Receipt {
    serde_json::from_value(json!({
        "status":"0x1","blockNumber":row["logs"][0]["blockNumber"],
        "effectiveGasPrice":"0x0","gasUsed":format!("0x{:x}",row["withFrame"]["gas"].as_u64().unwrap()),
        "logs":row["logs"],
    })).unwrap()
}

const USDC: Address = address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
const USDT: Address = address!("dac17f958d2ee523a2206206994597c13d831ec7");

fn archive(input: Address, output: Address) -> (Value, Address, Address) {
    let data = file(include_bytes!("../../tests/data/first_run_dex_2026-11-26/taurus_ethereum_bound_v2.json.gz"));
    let roles = file(include_bytes!("../../tests/data/first_run_dex_2026-11-26/taurus_ethereum_wallets.json.gz"));
    let frame = data["builders"].as_array().unwrap().iter().find(|b| b["builder"] == "Titan").unwrap();
    let row = frame["rows"].as_array().unwrap().iter().find(|row| row["withFrame"]["status"] == "paid"
        && row["input"].as_str().unwrap().parse::<Address>().ok() == Some(input)
        && row["output"].as_str().unwrap().parse::<Address>().ok() == Some(output)).unwrap();
    (row.clone(), roles["owner"].as_str().unwrap().parse().unwrap(), roles["module"].as_str().unwrap().parse().unwrap())
}

#[test]
fn independent_taurus_frames_pay_the_recorded_owner_and_fund_the_recorded_module() {
    let (buy, owner, _) = archive(USDC, USDT);
    assert_eq!(settlement_swap::amount(&receipt(&buy), owner, USDT, Side::Sell, 6), Some(Decimal::new(5_004_680, 6)));
    let (sell, payer, module) = archive(USDT, USDC);
    assert_eq!(owner, payer);
    assert_eq!(fill::moved(&receipt(&sell), USDT, Some(payer), Some(module)), U256::from(5_000_000));
}

// Следующие ответы задаёт тестовый получатель. Вывод и зачисление без комиссии; заявленная
// сумма продажи исполняется целиком. Эти ответы не являются наблюдением биржевой сделки.
#[derive(Default)]
struct Exit { withdrawal: Option<Decimal>, sale: Option<Decimal> }

impl Exit {
    fn withdraw(&mut self, buy: &TradeResult, from: &VenueId, to: &VenueId, amount: Decimal) -> (Transfer, Transfer) {
        assert!(self.withdrawal.replace(amount).is_none());
        let sent = Transfer { venue:from.clone(), asset:buy.request.asset.clone(), network:"Ethereum".into(),
            amount, status:TransferStatus::Sent, venue_ref:"test-withdrawal".into(),
            tx_hash:None, fee:None, updated_at:buy.finished_at };
        let credited = Transfer {venue:to.clone(), status:TransferStatus::Credited, venue_ref:"test-credit".into(), ..sent.clone()};
        (sent, credited)
    }

    fn sell(&mut self, buy: &TradeResult, venue: &VenueId, amount: Decimal) -> TradeResult {
        assert!(self.sale.replace(amount).is_none());
        TradeResult { request:TradeRequest{venue:venue.clone(),side:Side::Sell,qty:amount,..buy.request.clone()},
            filled_qty:amount, chain:None, venue_ref:"test-sale".into(), ..buy.clone() }
    }
}

/// Вызывается из теста квитанций с результатом рабочего recovery_order::spot.
pub(crate) async fn exit(spot: SpotFill) {
    let buy = spot.trade.clone();
    let venue = buy.request.venue.clone();
    let dest = VenueId("test-receiver".into());
    let (cash, qty) = super::settlement_target::amounts(&spot, &buy.request.asset, "USDC").unwrap();
    assert_eq!(cash, Decimal::new(5,0));
    let mut deal = Deal::new(buy.request.deal_id.clone(), Direction{ asset:buy.request.asset.clone(),
        buy_on:venue.clone(), sell_on:dest.clone()}, Value::Null, Decimal::ZERO);
    let core = open("slot-overfill").await;
    let mut exit = Exit::default();
    fills::trade(&core, &mut deal, &buy).await;
    assert_eq!(deal.qty, Decimal::new(5_004_680,6));
    assert!(deal.qty > buy.request.qty);
    let (sent, credited) = exit.withdraw(&buy, &venue, &dest, deal.qty);
    fills::transfer(&core, &mut deal, &sent, &credited, &dest, buy.avg_price).await;
    // Даже если на принимающем счёте лежит дополнительная монета, она не принадлежит сделке.
    assert_eq!(remain_fact::settle_qty(deal.qty, qty + Decimal::ONE), qty);
    let sale = exit.sell(&buy, &dest, deal.qty);
    fills::trade(&core, &mut deal, &sale).await;
    assert_eq!((exit.withdrawal, exit.sale), (Some(qty), Some(qty)));
    assert_eq!(deal.qty, Decimal::ZERO);
}

/// На BSC обычное ядро сохраняет фактический кредит записанного межплощадочного перевода.
#[tokio::test]
async fn bsc_transfer_uses_the_recorded_credit_after_withdrawal_fee() {
    let row: Value = serde_json::from_str(include_str!(
        "../../tests/data/poloniex_withdraw_alias_2026-11-23/ledger.json")).unwrap();
    let sent: Transfer = serde_json::from_value(row["sent"].clone()).unwrap();
    let credit: Transfer = serde_json::from_value(row["credited"].clone()).unwrap();
    assert_eq!(sent.tx_hash, credit.tx_hash);
    let mut deal = Deal::new(sent.venue_ref.clone(), Direction { asset:credit.asset.clone(),
        buy_on:sent.venue.clone(), sell_on:credit.venue.clone() }, Value::Null, Decimal::ZERO);
    deal.qty = sent.amount;
    let core = open("slot-bsc-credit").await;
    fills::transfer(&core, &mut deal, &sent, &credit, &credit.venue, Decimal::ZERO).await;
    assert_eq!(deal.qty, credit.amount);
    assert!(deal.qty < sent.amount);
}

async fn open(label: &str) -> super::engine::Core {
    let path = std::env::temp_dir().join(format!("arb-{label}-{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
    std::fs::create_dir_all(&path).unwrap();
    super::test_core::open(&path).await
}

/// Отдельная проверка штатных мест передачи аргументов: тестовый получатель выше не выполняет POST.
#[test]
fn outgoing_requests_use_the_owned_position_and_the_confirmed_credit() {
    let code = |source: &str| source.lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .flat_map(str::split_whitespace)
        .collect::<String>();
    assert!(code(include_str!("executor.rs")).contains(
        "transfers::send_trade(core,&deal.id,(&d.buy_on,&d.sell_on),&d.asset,deal.qty,&mutgas,)"
    ));
    assert!(code(include_str!("sell.rs")).contains(
        "trade_ops::trade_seen(core,&deal.id,&v,&a,Side::Sell,deal.qty,"
    ));
    let cycle = code(include_str!("settlement_cycle.rs"));
    assert!(cycle.contains("let(buy_debit,bought)=target::amounts(&buy,"));
    assert!(cycle.contains("settlement_transfer::move_token(core,&deal.id,(m,&d.asset),wallet,!funding,bought,)"));
    assert!(cycle.contains("settlement_request::request(core,&deal.id,p,Side::Sell,moving.credit.amount,"));
    // Решение Тиграна 28.11.2026: перевод зачисленного на торговый — после move_token и до заявки продажи;
    // в пути возврата — до IOC USDC → USDT.
    let moved = at_in(&cycle, "settlement_transfer::move_token(");
    let to_trading = at_in(&cycle, "settlement_to_trading::ensure(core,on,to,&d.asset,&p.target_cex,&moving.credit)");
    let sell = at_in(&cycle, "settlement_request::request(core,&deal.id,p,Side::Sell,");
    assert!(moved < to_trading && to_trading < sell);
    let path = code(include_str!("settlement_path.rs"));
    let ensure = at_in(&path, "settlement_to_trading::ensure(");
    let convert = at_in(&path, "settlement_cash::convert(core,&p.deal,&market,Side::Sell,");
    assert!(ensure < convert);
}

/// Позиция фрагмента в тексте кода (без пробелов и комментариев). Паника — фрагмента нет.
fn at_in(code: &str, needle: &str) -> usize {
    code.find(needle).unwrap_or_else(|| panic!("нет {needle}"))
}
