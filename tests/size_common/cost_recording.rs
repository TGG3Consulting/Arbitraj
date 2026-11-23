//! Источник для сравнения размеров на одном снимке CAKE/USDT, BSC и BingX.
//! Записанный газ двух чужих транзакций задаёт воспроизводимый сценарий расходов.
//! Вывод с биржи и возврат на неё используют стандартные пределы, округление и комиссии costs.rs.

use alloy_primitives::U256;
use arb::core::convert::Rates;
use arb::core::costs::Source;
use arb::core::dirctx::DirCtx;
use arb::core::size::Fail;
use arb::core::types::{Cost, CostKind, Direction, PoolState, Quote};
use arb::venues::bingx::{book_print, symbols};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::sieve_common::Src;
use crate::size_common::{PAIR, Rec, d, json_file};

pub struct Gas {
    pub swap: Decimal,
    pub transfer: Decimal,
    pub bnb_usdt: Decimal,
}

/// Уровни биржи и резервы V2 пришли в одну секунду. Проверяются также адрес и блок источника.
pub async fn recording() -> (Rec, Gas) {
    let record = json_file("tests/data/size_cost_replay_2026-11-23.json");
    let bsc = &record["bsc"];
    assert_eq!(bsc["pair"], format!("{PAIR:#x}"));
    let block_time = bsc["timestamp"].as_u64().unwrap();
    let book = |symbol: &str| {
        let data = &record[symbol]["data"];
        assert_eq!(data["ts"].as_u64().unwrap() / 1000, block_time);
        book_print::parse(data, 20).unwrap()
    };
    let cake = book("CAKE-USDT");
    let bnb_usdt = book("BNB-USDT").asks[0].price;
    let mut rec = Rec::load("full-cost-volume").await;
    // Фикс вывода CAKE по более позднему записанному плану BingX, 03.11.
    rec.net.withdraw_fee = Some(d("0.011"));
    rec.asks = cake.asks;
    rec.bids = cake.bids;
    let rules = symbols::Symbols::default();
    rules.load(
        symbols::parse_list(&json!({"symbols": [record["symbol"]["data"]]}), "USDT").unwrap(),
        Utc::now(),
    );
    rec.pair = rules.state("CAKE").unwrap();
    assert!(rec.pair.online && rec.pair.api_buy && rec.pair.api_sell);
    let reserves = bsc["reserves"]
        .as_str()
        .unwrap()
        .strip_prefix("0x")
        .unwrap();
    let word = |n: usize| U256::from_str_radix(&reserves[n * 64..(n + 1) * 64], 16).unwrap();
    rec.pool.pool.state = PoolState::V2 {
        reserve0: word(0),
        reserve1: word(1),
    };
    let block =
        u64::from_str_radix(bsc["block"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
    rec.pool.pool.block = block;
    rec.pool.synced_block = block;
    let paid = |r: &Value| {
        Decimal::from(r["gasUsed"].as_u64().unwrap())
            * Decimal::from(r["effectiveGasPrice"].as_u64().unwrap())
            / d("1000000000000000000")
    };
    let gas = &record["gas_examples"];
    (
        rec,
        Gas {
            swap: paid(&gas["swap"]),
            transfer: paid(&gas["transfer"]),
            bnb_usdt,
        },
    )
}

pub struct Priced<'a> {
    /// Первый запрос котировки A на каждом оцененном размере, включая отказы по ограничениям.
    pub buy_quotes: AtomicU32,
    pub sell_quotes: AtomicU32,
    pub source: Src<'a>,
    pub gas: &'a Gas,
    pub dex_slippage_pct: Decimal,
}

impl Source for Priced<'_> {
    async fn quote(&self, dir: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        if sell {
            self.sell_quotes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.buy_quotes.fetch_add(1, Ordering::Relaxed);
        }
        let mut quote = self.source.quote(dir, sell, qty).await?;
        if (sell && self.source.dex_b) || (!sell && self.source.dex_a) {
            quote
                .costs
                .push(Cost::new(CostKind::Gas, self.gas.swap, "BNB", false));
        }
        Ok(quote)
    }

    fn worst(
        &self,
        dir: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        if (sell && self.source.dex_b) || (!sell && self.source.dex_a) {
            let slip = self.dex_slippage_pct / Decimal::ONE_HUNDRED;
            return Ok(avg
                * if sell {
                    Decimal::ONE - slip
                } else {
                    Decimal::ONE + slip
                });
        }
        self.source.worst(dir, sell, qty, avg)
    }

    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        if self.source.dex_a {
            Ok(vec![Cost::new(
                CostKind::TransferGas,
                self.gas.transfer,
                "BNB",
                false,
            )])
        } else {
            self.source.withdraw(cx, amount).await
        }
    }

    async fn ret(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        if self.source.dex_b {
            Ok(vec![Cost::new(
                CostKind::TransferGas,
                self.gas.transfer,
                "BNB",
                false,
            )])
        } else {
            self.source.ret(cx, amount).await
        }
    }

    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        price: Decimal,
        _must: &[Cost],
    ) -> Result<Rates, Fail> {
        Ok(Rates {
            quote: cx.quote.clone(),
            coin: coin.into(),
            coin_price: price,
            prices: HashMap::from([("BNB".into(), self.gas.bnb_usdt)]),
        })
    }
}
