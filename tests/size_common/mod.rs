//! Файл: tests/size_common/mod.rs
//! Что делает: кривая «прибыль от объёма» CAKE на записанных данных для проверки подбора объёма (решение 66):
//! - стакан BingX CAKE-USDT@depth20 — tests/data/bingx/ws_spot_2026-10-23.jsonl (разбор адаптера depth::parse);
//! - правила пары CAKE-USDT (stepSize 0.001, minNotional, maxNotional) — tests/data/bingx_2026-10-24/symbols.json;
//! - сеть BEP20 CAKE у BingX (withdrawFee, withdrawMin, depositMin) — tests/data/bingx_2026-10-24/config_getall.json;
//! - резервы пула CAKE/USDT PancakeSwap V2 0xa39a…755b (token0 CAKE, token1 USDT) — getReserves на блоке 0x7b936ae,
//!   tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json; математика пула — адаптера (math/v2.rs).
//!
//! Затраты кривой — только те, что есть в записях: проскальзывание по уровням стакана и по пулу, комиссия пула
//! (dex.v2_fee_pct — 0,25% по документации PancakeSwap V2, как настройка теста), комиссия вывода, минимумы, шаг,
//! сумма ордера. Ставки taker аккаунта и цены газа в записях нет — в кривую не входят (поиск от них не зависит).
//! Записей WFI и DEOD нет — живой случай «вложение на бюджете чуть больше бюджета» повторён на CAKE.
//! Кто вызывает: tests/core_size_optimum.rs.
//! От чего зависит: arb::core::size, arb::core::costs_quote, arb::venues::bingx::{book, depth}, arb::venues::bsc_dex.

use std::str::FromStr;

use alloy_primitives::U256;
use arb::core::costs_quote::{floor_step, notional};
use arb::core::size::{Curve, Fail, Point};
use arb::core::types::PriceLevel;
use arb::gateway::events::SpotPairState;
use arb::venues::bingx::book::walk;
use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bsc_dex::math::v2;
use arb::venues::bsc_dex::num::{to_decimal, to_units};
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::Value;

/// Десятичное из строки.
pub fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Всё записанное о CAKE.
pub struct Rec {
    pub asks: Vec<PriceLevel>,
    pub bids: Vec<PriceLevel>,
    pub pair: SpotPairState,
    pub withdraw_fee: Decimal,
    pub withdraw_min: Decimal,
    pub deposit_min: Decimal,
    pub r_cake: U256,
    pub r_usdt: U256,
    pub fee: (U256, U256),
}

/// Читает JSON файла. Принимает: путь. Паника — файла нет (тест).
fn json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Число из поля JSON (строка или число, как прислал BingX).
fn num(v: &Value) -> Decimal {
    match v {
        Value::String(s) => d(s),
        other => d(&other.to_string()),
    }
}

impl Rec {
    /// Загружает записи CAKE.
    pub fn load() -> Rec {
        let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
        let (mut asks, mut bids) = (vec![], vec![]);
        for l in raw.lines() {
            let m = serde_json::from_str::<Value>(l).unwrap()["m"]
                .as_str()
                .unwrap()
                .to_string();
            if let Msg::Depth {
                symbol,
                asks: a,
                bids: b,
                ..
            } = depth::parse(&m)
                && symbol == "CAKE-USDT"
            {
                (asks, bids) = (a, b);
            }
        }
        let sym = json("tests/data/bingx_2026-10-24/symbols.json");
        let s = sym["data"]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["symbol"] == "CAKE-USDT")
            .unwrap();
        let pair = SpotPairState {
            ticker: "CAKE".into(),
            status: s["status"].as_i64().unwrap(),
            online: true,
            api_buy: s["apiStateBuy"].as_bool().unwrap(),
            api_sell: s["apiStateSell"].as_bool().unwrap(),
            step_size: num(&s["stepSize"]),
            min_notional: Some(num(&s["minNotional"])),
            max_notional: Some(num(&s["maxNotional"])),
        };
        let cfg = json("tests/data/bingx_2026-10-24/config_getall.json");
        let coin = cfg["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["coin"] == "CAKE")
            .unwrap();
        let net = coin["networkList"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["network"] == "BEP20")
            .unwrap();
        let node = json("tests/data/indexers_2026-10-24/arb_node_responses_3_2026-10-25.json");
        let res = node["results"].as_array().unwrap().iter()
            .find(|r| r["name"] == "s1 CAKE pancakeswap_v2 0xa39af17ce4a8eb807e076805da1e2b8ea7d0755b getReserves")
            .unwrap();
        let hex = res["response"]["result"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x")
            .to_string();
        let word = |i: usize| U256::from_str_radix(&hex[i * 64..(i + 1) * 64], 16).unwrap();
        Rec {
            asks,
            bids,
            pair,
            withdraw_fee: num(&net["withdrawFee"]),
            withdraw_min: num(&net["withdrawMin"]),
            deposit_min: num(&net["depositMin"]),
            r_cake: word(0),
            r_usdt: word(1),
            fee: v2::fee_ratio(d("0.25")).unwrap(),
        }
    }

    /// Цена пула USDT за CAKE без комиссии (оценка «бюджет / лучшая цена» у DEX).
    pub fn pool_price(&self) -> Decimal {
        to_decimal(self.r_usdt, 18).unwrap() / to_decimal(self.r_cake, 18).unwrap()
    }
}

/// Направление сделки.
#[derive(Clone, Copy, Debug)]
pub enum Dir {
    /// Купить в пуле, перевести на BingX, продать по bids (как живой случай WFI bsc_dex → bingx).
    DexToBingx,
    /// Купить по asks BingX, вывести с BingX, продать в пул.
    BingxToDex,
}

/// Кривая по записям.
pub struct RecCurve<'a> {
    pub rec: &'a Rec,
    pub dir: Dir,
}

#[async_trait]
impl Curve for RecCurve<'_> {
    type Data = ();

    async fn at(&mut self, q: Decimal) -> Result<Point<()>, Fail> {
        let r = self.rec;
        let p = Some(&r.pair);
        let units = |x: Decimal| to_units(x, 18).unwrap();
        let dec = |x: U256| to_decimal(x, 18).unwrap();
        let (invested, arrive, proceeds) = match self.dir {
            Dir::DexToBingx => {
                let usdt_in = v2::amount_in(units(q), r.r_usdt, r.r_cake, r.fee)
                    .ok_or_else(|| Fail::TooLarge("в пуле столько нет".into()))?;
                if q < r.deposit_min {
                    return Err(Fail::TooSmall(format!(
                        "приход {q} меньше депозита {}",
                        r.deposit_min
                    )));
                }
                let sell = floor_step(q, p);
                let (filled, sum) = walk(&r.bids, sell);
                if filled < sell {
                    return Err(Fail::TooLarge("глубины bids не хватает".into()));
                }
                notional("bingx", p, sum)?;
                (dec(usdt_in), q, sum)
            }
            Dir::BingxToDex => {
                let (filled, sum) = walk(&r.asks, q);
                if filled < q {
                    return Err(Fail::TooLarge("глубины asks не хватает".into()));
                }
                notional("bingx", p, sum)?;
                if q < r.withdraw_min {
                    return Err(Fail::TooSmall(format!(
                        "вывод {q} меньше минимума {}",
                        r.withdraw_min
                    )));
                }
                let arrive = q - r.withdraw_fee;
                let out = v2::amount_out(units(arrive), r.r_cake, r.r_usdt, r.fee)
                    .ok_or_else(|| Fail::TooLarge("пул не считает".into()))?;
                (sum, arrive, dec(out))
            }
        };
        Ok(Point {
            qty: q,
            invested,
            arrive,
            profit: proceeds - invested,
            data: (),
        })
    }
}
