//! Файл: venues/bsc_dex/venue.rs
//! Что делает: адаптер DEX в сети BSC — реализация контракта gateway/contract.rs (функции контракта; общее состояние —
//! parts.rs). Срочность запросов к узлу (решение 61, gateway/urgency.rs): сделка, вывод, сверки перевода и стоимость
//! вывода — Trade; котировка — класс ядра или Quote; остальное — фон.
//! Кто вызывает: main.rs (через mod.rs: создаёт и регистрирует в реестре шлюза).
//! От чего зависит: файлы этой папки, gateway, settings.
//! Настройки: docs/SETTINGS.md — «Узел BSC», «DEX», «Отправка транзакций», venue.* и feed.* с областью bsc_dex;
//! exec.via_kyber, exec.dex_best_of (решение 123, «лучший из двух» — best_of/).
//! Решение 132: деньги и адрес кошелька — только у вида на счёт (venue_account.rs); без привязки — отказ (Ctx::bound).

use async_trait::async_trait;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::core::types::{
    Asset, Balance, CoinNetworkInfo, Cost, DexAccountId, PoolDirectory, Quote, SendChannelInfo,
    Side, TradeRequest, TradeResult, Transfer, VenueId, WalletQueue, WithdrawRequest,
};
use crate::gateway::contract::{ConnectionCheck, DexFamily, Venue, VenueKind};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::events::MarketEvent;

use super::BscDex;
use super::{
    coins, connection, directory, exec, pools, quote, resolve, resolve_withdraw, send, wallet,
};
use crate::gateway::urgency::{self, Urgency as Class, scope};

#[async_trait]
impl Venue for BscDex {
    /// Идентификатор площадки (`bsc_dex`). Ошибок нет.
    fn id(&self) -> VenueId {
        self.p.ctx.venue()
    }

    /// Вид площадки — DEX. Ошибок нет.
    fn kind(&self) -> VenueKind {
        VenueKind::Dex
    }

    /// Решение 132: вид адаптера на кошелёк счёта — venue_account.rs. Ошибки: NotConfigured — нет кошелька.
    fn for_account(&self, account: &DexAccountId) -> GatewayResult<std::sync::Arc<dyn Venue>> {
        self.on_account(account)
    }

    /// «Проверить подключение»: узел HTTP (eth_chainId = 56, eth_blockNumber), узел WebSocket (newHeads), кошелёк,
    /// eth_simulateV1 — connection.rs (execution-apis src/eth, коммит 16caee27). Ошибок нет: причина — в ответе.
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck> {
        Ok(connection::check(&self.p.ctx).await)
    }

    /// Монеты DEX с достоверными пулами (адрес токена — из пула, сеть BSC) — coins.rs. Ошибки: NotConfigured.
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>> {
        coins::networks(&self.p.ctx, &self.p.registry)
    }

    /// Подписка на рынок: опрос блоков (решение 57), пулы монет (eth_subscribe logs, execution-apis subscribe.yaml), новые пулы,
    /// циклы проверок; события — в `out` (pools/market.rs). Прежние задачи подписки останавливаются.
    /// Ошибки: NotConfigured (venue.quote_asset, dex.*), узел.
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()> {
        // Решение 132: подписка — только у основного адаптера; вид на счёт рынок не слушает.
        if self.is_bound() {
            return Err(self.p.ctx.err(
                ErrorCode::NotApplicable,
                "подписка на рынок — у адаптера DEX без привязки к кошельку",
            ));
        }
        // Исполнению через KyberSwap (exec.via_kyber) нужен тот же набор монет (адрес USDT и монет) — отдаём его
        // контексту KS. KyberSwap как площадку-цены не подписываем: цены и сигналы идут с bsc_dex.
        self.kyber.set_assets(assets.clone());
        self.subs.subscribe(&self.p, assets, out).await
    }

    /// Котировка на объём по пулам (локальная математика по официальным библиотекам — quote/, math/).
    /// Ошибки: StaleData, CoinNotChecked / CoinCheckFailed, NoRoute, NotConfigured, узел.
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote> {
        // Решение 95: финальная котировка расчёта кандидата (ядро пересчитывает найденный оптимум) при exec.via_kyber —
        // цена KyberSwap, того, кто исполнит обмен; по ней ядро решает «торговать или нет». У KyberSwap нет маршрута
        // (пул, которого он не знает) — наша котировка: исполнит наш собственный сборщик (откат в trade()).
        if crate::gateway::quote_stage::is_final()
            && self.p.ctx.flag("exec.via_kyber").unwrap_or(false)
        {
            // Решение 123: «лучший из двух» — KyberSwap и своя параллельно, берётся лучшая (best_of/quote.rs).
            if super::best_of::enabled(&self.p.ctx) {
                return super::best_of::quote::quote(&self.p, &self.kyber, asset, side, qty).await;
            }
            match crate::venues::kyberswap::quote::quote(&self.kyber, asset, side, qty).await {
                Err(e) if e.code == ErrorCode::NoRoute => {}
                Err(e) => return Err(e),
                Ok(mut q) => {
                    // Котировка от имени площадки bsc_dex: ядро сверяет площадку стороны.
                    q.venue = self.p.ctx.venue();
                    return Ok(q);
                }
            }
        }
        // Котировка: ядро на пути сделки ставит Trade (расчёт кандидата, предпроверка); иначе — Quote (живой экран,
        // ниже проб налогов, выше фона).
        let class = urgency::is_set().unwrap_or(Class::Quote);
        scope(class, self.p.quoting().quote(asset, side, qty)).await
    }

    /// Решение 217: снимок котировки на момент решения — `Arc` записей пулов, разрешения, блок (frozen.rs). Только
    /// память. Ошибки: StaleData (блок, лента событий), NotConfigured (тикер USDT), NoRoute (нет адреса монеты).
    async fn quote_snapshot(
        &self,
        asset: &Asset,
        side: Side,
    ) -> GatewayResult<crate::gateway::frozen::QuoteSnapshot> {
        Ok(std::sync::Arc::new(super::frozen::take(
            self.p.quoting(),
            asset,
            side,
        )?))
    }

    /// Обмен через Universal Router своей DEX (прямой) или PancakeSwap Infinity (умный) — exec/trade.rs; источники —
    /// research/flow/bsc_dex.md, раздел 5, bsc_smart_router.md. Ошибки: см. exec/trade.rs (с фактом транзакции).
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        // Решение 222: монета сделки — по изменению баланса кошелька на блоке свопа (все пути исполнения ниже).
        let r = self.trade_logs(req).await;
        let fact = exec::balance_fact::apply(&self.p.ctx, &self.p.tokens, r);
        // Решение 231: «исход неизвестен» с фактом транзакции — «чем найти» (хеш, nonce) для ядра (resolve.rs).
        scope(Class::Trade, fact).await.map_err(resolve::attach)
    }

    /// Перевод монеты из кошелька на адрес (ERC-20 transfer, EIP-20) — wallet/transfer.rs.
    /// Ошибки: InvalidAddress, NotConfigured, каналы, TxNotIncluded / WithdrawRejected (с фактом транзакции).
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        let env = self.p.env();
        scope(
            Class::Trade,
            wallet::transfer::withdraw(&env, &self.p.tokens, &self.p.nonces, req),
        )
        .await
        .map_err(resolve::attach)
    }

    /// Статус перевода из кошелька по квитанции (execution-apis transaction.yaml#L127-L165) — wallet/transfer_status.rs.
    /// Ошибки: узел, хеш не 32 байта, BadResponse.
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        let w = wallet::signer::address(&self.p.ctx)?;
        let work =
            wallet::transfer_status::withdrawal_status(&self.p.ctx, &self.p.tokens, venue_ref, w);
        scope(Class::Trade, work).await
    }

    /// Решение 231: своп с неизвестным исходом — квитанция по хешу; очередь кошелька снимается, итог свопа — по
    /// контексту разбора из «чем найти» (resolve.rs, exec/swap_rebuild.rs; нет контекста — человек); монета сделки — по
    /// балансу кошелька на блоке свопа (решение 222, как у trade).
    async fn resolve_trade(
        &self,
        req: TradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        self.p.ctx.bound()?;
        let work = async {
            match resolve::trade(&self.p, req, p).await? {
                Some(t) => exec::balance_fact::apply(&self.p.ctx, &self.p.tokens, Ok(t))
                    .await
                    .map(Some),
                None => Ok(None),
            }
        };
        scope(Class::Trade, work).await
    }

    /// Решение 231: перевод из кошелька с неизвестным исходом — квитанция по хешу; хеши отмен известны — и их квитанции
    /// и nonce кошелька (resolve_withdraw.rs).
    async fn resolve_withdraw(
        &self,
        req: WithdrawRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<Transfer>> {
        self.p.ctx.bound()?;
        scope(Class::Trade, resolve_withdraw::withdraw(&self.p, req, p)).await
    }

    /// Адрес кошелька; заодно регистрирует ожидаемое поступление монеты (после конца идущего обмена с ней на выходе) —
    /// wallet/inflows.rs. Ошибки: ключ кошелька, NotConfigured (у монеты нет адреса в BSC).
    async fn deposit_address(&self, asset: &Asset, _network: &str) -> GatewayResult<String> {
        // Решение 132: адрес депозита — кошелька счёта сделки; без счёта не отдаётся (монеты не уйдут не туда).
        self.p.ctx.bound()?;
        let w = wallet::signer::address(&self.p.ctx)?;
        let token = wallet::transfer::token(&self.p.ctx, asset)?;
        self.p.inflows.expect(token).await;
        Ok(w.to_string())
    }

    /// Статус зачисления; любое завершение снимает ожидаемое поступление — wallet/deposit.rs.
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer> {
        self.p.ctx.bound()?;
        scope(
            Class::Trade,
            wallet::deposit::status(&self.p, asset, tx_hash),
        )
        .await
    }

    /// Вывод на кошелёк не состоялся — ожидание поступления снято (wallet/deposit.rs). Ошибок нет.
    async fn deposit_abandoned(&self, asset: &Asset) -> GatewayResult<()> {
        self.p.ctx.bound()?;
        wallet::deposit::abandoned(&self.p, asset).await;
        Ok(())
    }

    /// Адрес кошелька счёта (решение 132) — wallet/signer.rs; только у вида на счёт. Ошибки: счёт не выбран, ключ.
    async fn wallet_address(&self) -> GatewayResult<String> {
        self.p.ctx.bound()?;
        Ok(wallet::signer::address(&self.p.ctx)?.to_string())
    }

    /// Очередь транзакций кошелька (wallet/nonce.rs). Ошибок нет.
    async fn wallet_queue(&self) -> GatewayResult<WalletQueue> {
        self.p.ctx.bound()?;
        Ok(wallet::deposit::queue(&self.p).await)
    }

    /// «Снять очередь после ручной проверки» — wallet/deposit.rs. Ошибки: NotApplicable — очередь не остановлена.
    async fn release_wallet_queue(&self, by: &str, pending_ok: bool) -> GatewayResult<WalletQueue> {
        self.p.ctx.bound()?;
        scope(
            Class::Trade,
            wallet::deposit::release(&self.p, by, pending_ok),
        )
        .await
    }

    /// Балансы кошелька: BNB (eth_getBalance) и известные токены (balanceOf, EIP-20) — wallet/balances.rs. Ошибки: узел, ключ.
    async fn balances(&self) -> GatewayResult<Vec<Balance>> {
        self.p.ctx.bound()?;
        wallet::balances::all(&self.p.ctx, &self.p.tokens).await
    }

    /// Баланс одного актива: 1–2 запроса к узлу вместо запроса по каждому токену (решение 113) — wallet/balances.rs.
    /// Ошибки: ключ кошелька, узел.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        self.p.ctx.bound()?;
        wallet::balances::single(&self.p.ctx, &self.p.tokens, asset).await
    }

    /// Стоимость перевода: газ (пробный перевод × цена газа) и налог на перевод — wallet/transfer.rs.
    /// Ошибки: CoinNotChecked / CoinCheckFailed, узел.
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>> {
        let env = self.p.env();
        scope(
            Class::Trade,
            wallet::transfer::cost(&env, &self.p.checks, asset, network, amount),
        )
        .await
    }

    /// Кнопка «Перепроверить»: налоги, крючки и сверка с квотером — только этих монет (check/recheck.rs; итоги —
    /// события CoinCheck / PoolCheck). Ошибок нет.
    async fn recheck_coins(&self, assets: Vec<Asset>) -> GatewayResult<()> {
        // Решение 211: монета — адрес контракта (у монеты без адреса пулов на DEX нет — проверять нечего).
        let coins: Vec<alloy_primitives::Address> =
            assets.into_iter().filter_map(|a| a.bsc_contract).collect();
        self.p.wake.coins(&coins);
        Ok(())
    }

    /// DEX с видом пулов из таблицы сопоставления (id области «DEX» = id DEX индексатора) — pools/families.rs; решение
    /// 128 — плюс DEX из справочника пулов и id с уже заданными настройками (вид None) — pools/dex_ids.rs. Ошибок нет.
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>> {
        Ok(pools::families::for_ui(&self.p.ctx))
    }

    /// Справочник пулов от индексатора (решение 58) — directory/. Ошибок нет: нет данных — пустые списки и причина.
    async fn pool_directory(&self) -> GatewayResult<PoolDirectory> {
        Ok(directory::snapshot(&self.p.ctx))
    }

    /// Пулы для экрана «Монеты» (решение 73) — pools/depth_facts.rs. Ошибки: USDT не известен, узел.
    async fn pool_depths(&self) -> GatewayResult<Vec<crate::core::types::PoolDepth>> {
        let p = &self.p;
        pools::depth_facts::all(&p.ctx, &p.registry, &p.tokens, &p.checks, None).await
    }

    /// Только перечисленные пулы (решение 74: пулы маршрута в карточке сигнала) — pools/depth_facts.rs.
    /// Ошибки: USDT не известен, узел.
    async fn pool_depths_of(
        &self,
        pools: &[crate::core::types::PoolRef],
    ) -> GatewayResult<Vec<crate::core::types::PoolDepth>> {
        let p = &self.p;
        pools::depth_facts::all(&p.ctx, &p.registry, &p.tokens, &p.checks, Some(pools)).await
    }

    /// Сигналы циклов DEX ↔ DEX (решение 85, фаза сигнала): поиск циклов (quote/cycle_scan.rs) и перевод в read-тип ядра
    /// (quote/cycle_view.rs). Направление за dex_dex.enabled; выключено — пусто (газ не трогаем). Только read-поверхность:
    /// сделок не запускает (их ведёт ядро, решение 124). Ошибки: StaleData (нет свежего блока), NotConfigured (настройки цикла), узел.
    async fn dex_cycles(&self) -> GatewayResult<Vec<crate::core::types::DexCycle>> {
        let q = self.p.quoting();
        if !quote::cycle_scan::enabled(&q.ctx) {
            return Ok(Vec::new());
        }
        // Цена газа — как при котировке и отправке: eth_gasPrice с надбавкой, не ниже минимума готовых каналов.
        let gas = q
            .gas
            .wei(&q.ctx, &q.heads)
            .await?
            .max(send::planned_floor(&q.ctx)?);
        // Решение 131: поиск циклов — счёт CPU, вне рабочих потоков tokio (quote/cpu.rs).
        let qc = q.clone();
        let signals = quote::cpu::run(
            &q.ctx,
            async move { quote::cycle_scan::scan(&qc, gas).await },
        )
        .await?;
        let venue = self.id();
        Ok(signals
            .iter()
            .map(|s| quote::cycle_view::to_dex_cycle(&venue, s))
            .collect())
    }

    /// «Сухая сделка DEX» (решение 121, C4): сборка своим сборщиком как для сделки и eth_simulateV1 от кошелька —
    /// exec/dry_swap.rs. Ничего не отправляет, nonce не занимает. Ошибок нет: отказ — причиной в итоге.
    async fn dex_dry_swap(
        &self,
        asset: Asset,
        side: Side,
        usdt: Decimal,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let q = self.p.quoting();
        // Класс Check (решение 61): ниже пути сделки, выше котировок живого экрана и фона.
        let mode = exec::executor::dry::DryMode::AsTrade;
        Ok(scope(
            Class::Check,
            exec::dry_swap::run(&q, asset, side, usdt, mode),
        )
        .await)
    }

    /// Решение 234: симуляция продажи до покупки — тем путём, кошельком счёта и минимумом, что у настоящей продажи
    /// (exec/presell.rs). Класс — как у котировки ядра (предпроверка сделки), иначе Check. Ничего не отправляет.
    /// Ошибки: см. exec/presell.rs.
    async fn dex_presell(
        &self,
        req: &TradeRequest,
    ) -> GatewayResult<crate::core::types::DexPresell> {
        let q = self.p.quoting();
        let class = urgency::is_set().unwrap_or(Class::Check);
        scope(class, exec::presell::run(&q, req.clone())).await
    }

    /// «Сухая сделка DEX» через исполнитель ArbExecutor с подставленным в симуляцию кодом (решение 122, часть X) —
    /// проверка исполнителя на живой сети до развёртывания. Ничего не отправляет. Ошибок нет: отказ — причиной в итоге.
    async fn dex_dry_swap_executor(
        &self,
        asset: Asset,
        side: Side,
        usdt: Decimal,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let q = self.p.quoting();
        let mode = exec::executor::dry::DryMode::Executor;
        Ok(scope(
            Class::Check,
            exec::dry_swap::run(&q, asset, side, usdt, mode),
        )
        .await)
    }

    /// Расчёт цикла DEX ↔ DEX на вход (решение 124): ноги по памяти реестра (quote/cycle_legs.rs), курсы и затраты —
    /// quote/cycle_cost.rs; with_wallet — ещё баланс базовой у кошелька. Ошибки: NoRoute, NotConfigured, узел.
    async fn dex_cycle_quote(
        &self,
        cycle: &crate::core::types::DexCycle,
        input: Decimal,
        with_wallet: bool,
    ) -> GatewayResult<crate::core::types::CycleQuote> {
        // Баланс базовой — кошелька счёта циклов (решение 132).
        if with_wallet {
            self.p.ctx.bound()?;
        }
        let q = self.p.quoting();
        let run = async {
            let legs = quote::cycle_legs::now(&q, cycle).await?;
            let r = quote::cycle_cost::rates(&q, cycle.base).await?;
            quote::cycle_cost::quote(&q, &legs, cycle.base, input, (r, with_wallet)).await
        };
        scope(Class::Quote, run).await
    }

    /// Сделка цикла DEX ↔ DEX (решение 124) — exec/cycle_trade.rs: свежее перечтение, исполнитель или бандл, проба,
    /// одна отправка без повторов. Ошибки: как у сделки DEX.
    async fn dex_cycle_trade(
        &self,
        req: crate::core::types::CycleTradeRequest,
    ) -> GatewayResult<crate::core::types::CycleTradeResult> {
        self.p.ctx.bound()?;
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(
            Class::Trade,
            exec::cycle_trade::trade(&q, &env, &self.p.nonces, req),
        )
        .await
    }

    /// Решение 233 (дополнение, п. 4): цикл DEX ↔ DEX с неизвестным исходом — тем же механизмом, что своп (resolve.rs:
    /// квитанция / бандл — resolve_bundle.rs; очередь кошелька снимается при выясненном исходе), итог цикла — с курсами
    /// сделки из «чем найти» (resolve_cycle.rs). Решение 222 не применяется — как у dex_cycle_trade.
    async fn resolve_cycle(
        &self,
        req: crate::core::types::CycleTradeRequest,
        p: &crate::gateway::pending::PendingRef,
    ) -> GatewayResult<Option<crate::core::types::CycleTradeResult>> {
        self.p.ctx.bound()?;
        scope(Class::Trade, super::resolve_cycle::cycle(&self.p, req, p)).await
    }

    /// Сухая сделка цикла (решение 124) — exec/cycle_dry.rs: сборка как у сделки и eth_simulateV1, ничего не уходит.
    /// Ошибок нет: отказ — причиной в итоге.
    async fn dex_dry_cycle(
        &self,
        req: crate::core::types::CycleTradeRequest,
        executor: bool,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let q = self.p.quoting();
        let mode = if executor {
            exec::executor::dry::DryMode::Executor
        } else {
            exec::executor::dry::DryMode::AsTrade
        };
        Ok(scope(Class::Check, exec::cycle_dry::run(&q, req, mode)).await)
    }

    /// Оценка развёртывания исполнителя ArbExecutor (решение 122, часть X): газ, цена, стоимость, будущий адрес — ничего не
    /// отправляет. Ошибки: ключ кошелька, каналы, узел.
    async fn dex_executor_quote(&self) -> GatewayResult<crate::core::types::ExecutorDeployQuote> {
        // Решение 132: исполнитель разворачивается кошельком, у которого нажата кнопка.
        self.p.ctx.bound()?;
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(Class::Check, exec::executor::deploy::quote(&q, &env)).await
    }

    /// Развёртывание исполнителя ArbExecutor с кошелька бота (решение 122, часть X) — только по подтверждению человека:
    /// SHA-256 байткода, который он видел в оценке. Ошибки: QuoteMismatch (байткод не тот), ключ, каналы, узел.
    async fn dex_executor_deploy(
        &self,
        sha256: String,
    ) -> GatewayResult<crate::core::types::ExecutorDeployed> {
        self.p.ctx.bound()?;
        let (q, env) = (self.p.quoting(), self.p.env());
        let run = exec::executor::deploy::deploy(&q, &env, &self.p.nonces, &sha256);
        scope(Class::Trade, run).await
    }

    /// «Проверить подключение» исполнителя ArbExecutor (ревью 122) — exec/executor/onchain.rs. Ошибок нет.
    async fn check_executor(&self) -> GatewayResult<ConnectionCheck> {
        Ok(exec::executor::onchain::probe(&self.p.ctx).await)
    }

    /// Каналы отправки (id области «канал») — send/channels.rs. Ошибок нет.
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>> {
        Ok(send::channels::ALL.iter().map(|c| c.info()).collect())
    }

    /// «Проверить подключение» канала: пустая транзакция — канал отвечает ошибкой JSON-RPC (send/check.rs; документация
    /// канала — в шапке его файла). Ошибки: неизвестный id.
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck> {
        send::check::check(&self.p.ctx, id).await
    }
}

impl BscDex {
    /// Сделка по пути исполнения (свой сборщик, KyberSwap, «лучший из двух»); факт — по событиям Transfer квитанции.
    /// Ошибки: см. exec/trade.rs (с фактом транзакции).
    async fn trade_logs(&self, req: TradeRequest) -> GatewayResult<TradeResult> {
        // Решение 132: сделка — только кошельком счёта (Ctx::bound).
        self.p.ctx.bound()?;
        // Исполнение через агрегатор KyberSwap (exec.via_kyber, решение 91): маршрут и своп строит KyberSwap по
        // объёму из расчёта (req.qty, req.worst_price); сигналы, цены и подбор размера — на bsc_dex. Нет маршрута у
        // KyberSwap (пул, которого он не знает) — откат на собственный исполнитель bsc_dex по маршруту котировки.
        if self.p.ctx.flag("exec.via_kyber").unwrap_or(false) {
            // Решение 123: «лучший из двух» — маршрут KyberSwap и свой в один момент, исполняет лучший (best_of/trade.rs).
            if super::best_of::enabled(&self.p.ctx) {
                return super::best_of::trade::trade(&self.p, &self.kyber, req).await;
            }
            match crate::venues::kyberswap::trade::trade(&self.kyber, req.clone()).await {
                Err(e) if e.code == ErrorCode::NoRoute => {}
                // Решение 121 (B7): исполнитель — в тексте ошибки, как у своего сборщика.
                Err(mut e) => {
                    e.message = format!("[KyberSwap] {}", e.message);
                    return Err(e);
                }
                other => return other,
            }
        }
        let (q, env) = (self.p.quoting(), self.p.env());
        scope(
            Class::Trade,
            exec::trade::trade(&q, &env, &self.p.nonces, req),
        )
        .await
    }
}
