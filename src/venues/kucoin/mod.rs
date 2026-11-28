//! Файл: venues/kucoin/mod.rs
//! Что делает: адаптер биржи KuCoin (спот, Classic API) — реализация контракта gateway/contract.rs (решение 137).
//! Всё общение с KuCoin — внутри этой папки; наружу — только типы ядра и единые ошибки шлюза.
//! Кто вызывает: main.rs (создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings, secrets, storage.
//! Настройки: venue.* с областью «kucoin», feed.stale_after_ms; секреты kucoin.api_key, kucoin.api_secret,
//! kucoin.api_passphrase.
//!
//! Только по официальной документации KuCoin (https://www.kucoin.com/docs-new/, раздел Classic; спецификации — SDK
//! github.com/Kucoin/kucoin-universal-sdk, spec/). Classic API — для классического счёта: «Continue using the Classic API
//! for Classic Account trading» (объявление KuCoin API Upgrade 2026-06-03). Счета: депозит — на main (funding), торговля —
//! на trade, вывод — только с main; переводы main ↔ trade — fund_move.rs. Ссылки — README.md этой папки.
//! Расчётный круг Base (USDT ↔ USDC): ордер — order_state.rs (inspect), вывод с меткой remark до POST и сверкой —
//! settle_withdraw.rs, зачисление — адрес депозита как в BSC (deposit_addr.rs), при MAIN — main → trade (решение 102).

pub mod account;
pub mod account_update;
pub mod all_balances;
pub mod all_balances_parse;
pub mod book;
pub mod book_print;
pub mod book_rest;
pub mod bullet;
mod cash_balance;
#[cfg(test)]
pub(crate) use cash_balance::upper as test_cash_upper;
pub mod cash_receipt;
pub mod ctx;
pub mod delist;
pub mod delist_ann;
pub mod deposit_addr;
pub mod deposits;
pub mod depth;
pub mod errors;
pub mod fast_check;
pub mod fast_log;
pub mod fast_state;
pub mod feed;
pub mod fees;
pub mod full_book;
pub mod fund_move;
pub mod incr;
pub mod local_book;
pub mod market;
pub mod market_sell;
pub mod networks_parse;
pub mod num;
pub mod order_rules;
pub mod order_state;
pub mod pairs;
pub mod pairs_watch;
pub mod reply;
pub mod resolve;
pub mod rest;
pub mod settle_withdraw;
pub mod sign;
pub mod subs;
pub mod symbols;
pub mod symbols_parse;
pub mod tap;
pub mod tap_decode;
pub mod trade;
pub mod transfers;
pub mod user_stream;
pub mod user_stream_conn;
pub mod wallet;
pub mod withdraw_quota;
pub mod ws;
pub mod ws_conn;
pub mod ws_fast;
pub mod ws_full;
pub mod ws_sess;

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, Quote, SendChannelInfo, Side,
    TradeRequest, TradeResult, Transfer, VenueId, WithdrawRequest,
};
use crate::gateway::contract::{AccountRole, ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::MarketEvent;
use crate::gateway::frozen::QuoteSnapshot;
use crate::gateway::tap::{TapData, TapInfo, TapSink, TapStream};

use book::Books;
use ctx::Ctx;
use deposit_addr::DepositAddrs;
use fees::Fees;
use subs::{Parts, Subs};
use symbols::Symbols;
use wallet::Networks;

/// Адаптер KuCoin.
pub struct Kucoin {
    ctx: Ctx,
    subs: Subs,
    symbols: Symbols,
    books: Books,
    fees: Fees,
    nets: Networks,
    addrs: DepositAddrs,
    cash_feed: cash_balance::Feed,
}

impl Kucoin {
    /// Создаёт адаптер. Ничего не подключает: соединения открываются по запросу ядра.
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            subs: Subs::default(),
            symbols: Symbols::default(),
            books: Books::default(),
            fees: Fees::default(),
            nets: Networks::default(),
            addrs: DepositAddrs::default(),
            cash_feed: Default::default(),
        }
    }

    /// Ответ «не применимо» для функций контракта только для DEX.
    fn not_applicable(&self, what: &str) -> GatewayError {
        self.ctx.tag(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!("KuCoin — биржа: {what} к ней не применимо"),
        ))
    }
}

#[async_trait]
impl Venue for Kucoin {
    /// GET сверяет окончательный ордер по ID из заявки. Открытый ордер остаётся на сверке.
    async fn inspect_settlement_order(
        &self,
        req: TradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        order_state::inspect(&self.ctx, req, p).await
    }
    fn settlement_account(&self) -> GatewayResult<String> {
        crate::gateway::settlement_intent::account(&self.ctx.vault, ctx::SECRET_API_KEY)
    }
    fn supports_cash_reservation(&self) -> bool {
        true
    }
    fn cash_opening_cap(
        &self,
        ticker: &str,
        currency: &str,
        qty: Decimal,
        worst: Decimal,
    ) -> Option<Decimal> {
        cash_balance::projected(
            &self.ctx,
            &self.fees,
            &format!("{ticker}-{currency}"),
            qty,
            worst,
        )
        .ok()
    }
    fn cash_order_cap(
        &self,
        req: &TradeRequest,
        p: &crate::gateway::pending::PendingRef,
        currency: &str,
    ) -> GatewayResult<Decimal> {
        cash_balance::cap(&self.ctx, &self.fees, req, p, currency)
    }
    async fn cash_identity(
        &self,
        currency: &str,
    ) -> GatewayResult<crate::storage::cex_funds::Bucket> {
        cash_balance::account_snapshot(&self.ctx, currency)
            .await
            .map(|(bucket, _, _)| bucket)
    }
    async fn cash_balance(
        &self,
        req: &TradeRequest,
        p: &crate::gateway::pending::PendingRef,
        currency: &str,
    ) -> GatewayResult<(crate::storage::cex_funds::Balance, rust_decimal::Decimal)> {
        cash_balance::read(&self.ctx, &self.fees, &self.cash_feed, req, p, currency).await
    }
    fn supports_settlement_order(&self) -> bool {
        true
    }
    /// Расчётный круг Base: метка попытки вывода (remark) сохраняется до POST, после потери ответа вывод ищется в
    /// истории выводов по ней (settle_withdraw.rs). У Withdraw (V3) нет клиентского ID — только remark, который
    /// возвращает Get Withdrawal History: https://www.kucoin.com/docs-new/api-3470146, https://www.kucoin.com/docs-new/api-3470145
    fn supports_settlement_withdrawal(&self) -> bool {
        true
    }
    /// Расчётный круг Base, решение Тиграна 28.11.2026: зачисление поддерживается безусловно. Адрес депозита —
    /// как в BSC (deposit_addr::for_deposit: TRADE — «TRADE (spot trading account)», Get Deposit Address V3
    /// https://www.kucoin.com/docs-new/api-3470140; MAIN — только при venue.deposit_to_spot = да). После зачисления
    /// ядро зовёт deposit_to_trading так же, как в BSC (core/transfer_credit.rs to_trading), и при адресе MAIN KuCoin
    /// переводит main → trade (решение 102, fund_move::to_trade, Flex Transfer https://www.kucoin.com/docs-new/api-3470147).
    fn supports_settlement_credit(&self) -> bool {
        true
    }
    /// Расчётный круг Base: создан ли вывод с потерянным ответом — по метке remark в истории выводов (settle_withdraw.rs).
    async fn resolve_withdraw(
        &self,
        req: WithdrawRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<Transfer>> {
        settle_withdraw::resolve(&self.ctx, req, p).await
    }
    /// Подготовленный символ проверяется по контракту спотового ордера. https://www.kucoin.com/docs-new/api-3470182
    fn matches_settlement_order(
        &self,
        p: &crate::gateway::pending::PendingRef,
        asset: &str,
        quote: &str,
    ) -> bool {
        p.symbol
            .as_deref()
            .is_some_and(|s| s == format!("{asset}-{quote}"))
    }

    fn id(&self) -> VenueId {
        self.ctx.venue()
    }

    fn kind(&self) -> VenueKind {
        VenueKind::Cex
    }

    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(account::check(&self.ctx).await)
    }

    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks(&self.ctx, &self.nets).await
    }

    /// Решение 113: у KuCoin есть запрос одной монеты (GET /api/v3/currencies/{currency}).
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        wallet::coin_networks_of(&self.ctx, ticker).await
    }

    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        let parts = Parts {
            ctx: self.ctx.clone(),
            cash_feed: self.cash_feed.clone(),
            symbols: self.symbols.clone(),
            books: self.books.clone(),
            fees: self.fees.clone(),
        };
        self.subs.subscribe(&parts, assets, out).await
    }

    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        self.books
            .quote(&self.ctx, &self.fees, &symbol, asset, side, qty)
            .await
    }

    /// Решение 217: снимок стакана монеты на момент решения (book.rs freeze — копия стакана из памяти, без запросов).
    /// Ошибки: как у котировки до прохода по уровням (стакана нет, устарел, комиссия, настройка).
    async fn quote_snapshot(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        let f = self
            .books
            .freeze(&self.ctx, &self.fees, &symbol, asset, side)?;
        Ok(std::sync::Arc::new(f))
    }

    /// Решение 238: снимок стакана по REST для ордера (стакан потока устарел) — book_print.rs snapshot
    /// (Get Part OrderBook), комиссия — тот же план, что у стакана потока (book.rs frozen).
    async fn quote_rest(&self, asset: &Asset, side: Side) -> GatewayResult<QuoteSnapshot> {
        let symbol = self.symbols.get(&self.ctx, &asset.ticker)?.symbol;
        let book = book_print::snapshot(&self.ctx, &symbol, asset).await?;
        let f = book::frozen(&self.ctx, &self.fees, &symbol, asset, side, book)?;
        Ok(std::sync::Arc::new(f))
    }

    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        trade::trade(&self.ctx, &self.symbols, req, None).await
    }

    /// Решение 268: рыночная продажа — market_sell.rs (POST /api/v1/hf/orders/sync, type market).
    async fn trade_market_sell(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        market_sell::sell(&self.ctx, &self.symbols, req).await
    }

    /// Решение 134: те же правила, что у настоящего ордера (order_rules::ready), и пробный ордер, если включён
    /// (order_rules::check). Решение 412: пробный — только здесь, перед настоящим ордером его нет.
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        order_rules::check(&self.ctx, &self.symbols, req).await
    }

    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        transfers::withdraw(&self.ctx, req).await
    }

    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        transfers::withdrawal_status(&self.ctx, venue_ref).await
    }

    /// Решение 231: итог ордера с неизвестным исходом — resolve.rs.
    async fn resolve_trade(
        &self,
        req: TradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        resolve::trade(&self.ctx, req, p).await
    }

    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String> {
        deposit_addr::for_deposit(&self.addrs, &self.ctx, asset, network).await
    }

    fn withdrawal_wire(
        &self,
        created: &Transfer,
        sent: &Transfer,
    ) -> Option<rust_decimal::Decimal> {
        // Запрос отправлен с feeDeductType INTERNAL. HTTP-запрос статуса привязан к withdrawalId.
        let fee = sent.fee.as_ref()?;
        (sent.status == crate::core::types::TransferStatus::Sent
            && !created.venue_ref.is_empty()
            && created.venue_ref == sent.venue_ref
            && created
                .asset
                .ticker
                .eq_ignore_ascii_case(&sent.asset.ticker)
            && fee.kind == crate::core::types::CostKind::WithdrawFee
            && fee.asset.eq_ignore_ascii_case(&created.asset.ticker)
            && fee.paid_in.is_none()
            && !fee.in_price
            && !fee.amount.is_sign_negative())
        .then(|| created.amount.checked_sub(fee.amount))
        .flatten()
        .filter(|n| *n > rust_decimal::Decimal::ZERO)
    }

    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        deposits::deposit_status(&self.ctx, asset, tx_hash).await
    }

    /// Решение 166: удержание вывода котируемой монеты — квота вывода (lockedAmount), withdraw_quota.rs.
    async fn withdraw_hold(&self, quote: &str) -> GatewayResult<crate::core::types::WithdrawHold> {
        withdraw_quota::withdraw_hold(&self.ctx, quote).await
    }

    /// Решение 144: все счета KuCoin для панели «Балансы» — all_balances.rs (только чтение).
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        all_balances::read(&self.ctx).await
    }

    /// Решение 226: делистинги — метка st (Special Treatment) пар GET /api/v2/symbols + анонсы delistings (delist.rs, delist_ann.rs).
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        delist::delistings(&self.ctx).await
    }

    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        account::balances(&self.ctx, None).await
    }

    /// Решение 113: баланс одной монеты — GET /api/v1/accounts?currency=&type=trade.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        Ok(account::balances(&self.ctx, Some(asset))
            .await?
            .into_iter()
            .find(|b| b.asset == asset))
    }

    /// Решение 102: депозит приходит на main — перевод на trade (venue.deposit_to_spot), fund_move.rs.
    /// Решение 174: роль счёта панели «Балансы» для кнопки «Продать» — названия те же, что в all_balances.rs: «Спот» (trade) и «Фонд (Funding)» (main); deposit_to_trading — main → trade (fund_move.rs), только если адрес депозита BSC зачисляет на main.
    fn account_role(&self, account: &str) -> AccountRole {
        match account {
            "Спот" => AccountRole::Trading,
            "Фонд (Funding)" => AccountRole::ToTrading,
            _ => AccountRole::Other,
        }
    }

    /// Решение 174: перевод со счёта ToTrading на торговый без условий deposit_to_trading (fund_move.rs).
    async fn move_to_trading(&self, asset: &Asset, amount: Decimal) -> GatewayResult<String> {
        fund_move::move_funds(
            &self.ctx,
            &asset.ticker,
            amount,
            fund_move::MAIN,
            fund_move::TRADE,
        )
        .await
    }

    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        fund_move::to_trade(&self.ctx, &self.addrs, asset, amount).await
    }

    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        _amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        wallet::withdraw_cost(&self.ctx, asset, network).await
    }

    async fn recheck_coins(&self, _assets: Vec<Asset>) -> GatewayResult<()> {
        Err(self.not_applicable("проверка налогов токена и крючков пулов"))
    }

    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Err(self.not_applicable("список видов пулов DEX"))
    }

    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Err(self.not_applicable("список каналов отправки транзакций"))
    }

    async fn check_send_channel(&self, _id: &str) -> GatewayResult<ConnectionCheck> {
        Err(self.not_applicable("проверка канала отправки транзакций"))
    }

    // Решение 125: запись окна — tap.rs (стакан — ws_conn.rs, сделки и изменения — feed.rs, разбор — tap_decode.rs).
    async fn window_tap(&self, asset: &Asset, sink: TapSink) -> GatewayResult<TapInfo> {
        self.symbols.ensure_fresh(&self.ctx).await?;
        tap::start(&self.ctx, &self.symbols, asset, sink)
    }

    async fn window_untap(&self, asset: &Asset) -> GatewayResult<()> {
        tap::stop(&self.ctx, asset);
        Ok(())
    }

    fn decode_tap(
        &self,
        stream: TapStream,
        data: &TapData,
    ) -> GatewayResult<crate::core::types::TapDecoded> {
        tap::decode(stream, data)
    }

    /// Решение 202 (для всех бирж, Тигран 08.11): свежесть соединений и пар стаканов (ws_conn.rs, gateway/feed_reg.rs).
    /// - scheduled = false: Orderbook - Level5 / Level50 (https://www.kucoin.com/docs-new/3470069w0, 3470070w0;
    ///   спецификация SDK spec/ws/openapi-spot-public.json) — «If there is no change in the market, data will not be
    ///   pushed»: тишина пары — не смерть;
    /// - per_pair = true: отписка — type «unsubscribe» (Classic WebSocket introduction; тот же вид — feed.rs) темы, в
    ///   которой пара подписана (до 100 пар; отписка одной пары из темы в прочитанной документации не описана —
    ///   docs/TASKS.md), см. ws_conn.rs;
    /// - confirm = true (решение 213, по содержимому): у кадра Level5/Level50 номера нет (data — asks, bids, timestamp);
    ///   сравниваются первые venue.depth_levels уровней ответа Get Part OrderBook (level2_20 / level2_100) и стакана в
    ///   памяти (book_print.rs, core/feed_print.rs).
    fn feed_health_for(
        &self,
        pair: Option<(&str, &str)>,
    ) -> Option<crate::gateway::feed_health::FeedHealth> {
        Some(self.books.feed.health_for(false, true, true, pair))
    }

    /// Решение 213: отпечатки стакана пары для подтверждения тихой пары по содержимому (book_print.rs). Символ — из
    /// справочника пар (symbols.rs). Ошибки: пары нет в справочнике, как у book_print::book_version.
    async fn book_version(
        &self,
        ticker: &str,
    ) -> GatewayResult<crate::gateway::feed_health::BookVersion> {
        let symbol = self.symbols.get(&self.ctx, ticker)?.symbol;
        book_print::book_version(&self.ctx, &self.books, &symbol, ticker).await
    }

    /// Решение 213: тихая пара подтверждена — стакан свежий в котировке до until_ms (book.rs, confirm).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        self.books.confirm(ticker, until_ms);
    }

    /// Решение 367 (повторное ревью t163, Б1): снять подтверждение тихой пары (book.rs unconfirm).
    fn book_unconfirmed(&self, ticker: &str) {
        self.books.unconfirm(ticker);
    }

    /// t79 (Тигран 10.11: согласовано): тихая пара «книга не менялась с переподписки» — уровни того же ответа REST
    /// стаканом пары (book_rest.rs put_rest: время кадра — получение ответа, свежесть — book.rs freeze).
    fn book_from_rest(
        &self,
        ticker: &str,
        levels: &crate::gateway::feed_health::BookPrint,
        sent_ms: i64,
        recv_ms: i64,
    ) -> GatewayResult<crate::core::types::OrderBook> {
        self.books
            .put_rest(self.id(), ticker, levels, sent_ms, recv_ms)
            .map_err(|why| {
                self.ctx.tag(crate::gateway::errors::GatewayError::new(
                    crate::gateway::errors::ErrorCode::StaleData,
                    None,
                    format!("KuCoin {ticker}: {why}"),
                ))
            })
    }

    /// Решение 202: переподписать пару в её соединении (unsubscribe + subscribe её темы, ws_conn.rs).
    async fn resubscribe_pair(&self, conn: &str, ticker: &str) -> GatewayResult<()> {
        use crate::gateway::feed_health::Cmd;
        self.books
            .feed
            .command(self.id(), conn, Cmd::Resub(ticker.to_string()))
    }

    /// Решение 202: переподключить одно соединение с теми же парами (ws.rs подключается заново).
    async fn reconnect_conn(&self, conn: &str) -> GatewayResult<()> {
        use crate::gateway::feed_health::Cmd;
        self.books.feed.command(self.id(), conn, Cmd::Reconnect)
    }
}
