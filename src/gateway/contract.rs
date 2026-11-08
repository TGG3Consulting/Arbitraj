//! Файл: gateway/contract.rs
//! Что делает: контракт адаптера площадки — набор функций, который обязан реализовать
//! каждый адаптер (биржа или DEX). Ядро видит площадки только через этот контракт.
//! Кто вызывает: шлюз (gateway/registry.rs). Реализуют: адаптеры в venues/.
//! От чего зависит: core/types, gateway/errors, gateway/events.
//! Настройки: у каждого адаптера свои (venue.<id>.*).
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).
//! Подробности — ARCHITECTURE.md, раздел «Контракт адаптера», и docs/ADAPTER_TEMPLATE.md.

use async_trait::async_trait;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::core::types::{
    AccountBalances, Asset, Balance, CoinNetworkInfo, Cost, PoolDirectory, PoolKind, Quote,
    SendChannelInfo, Side, TradeRequest, TradeResult, Transfer, VenueId, WalletQueue,
    WithdrawRequest,
};

use super::errors::{ErrorCode, GatewayError, GatewayResult};
use super::events::MarketEvent;

/// Вид площадки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VenueKind {
    /// Централизованная биржа (стакан, ордера, вывод и депозит).
    Cex,
    /// DEX в сети (пулы, свопы через свой кошелёк).
    Dex,
}

/// Решение 174: роль счёта биржи из account_balances для кнопки «Продать» панели «Балансы».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountRole {
    /// Счёт, на котором биржа торгует (balances/balance контракта, ордер trade): монета продаётся прямо отсюда.
    Trading,
    /// Счёт, с которого move_to_trading адаптера перекладывает монету на торговый (у BingX — фонд → спот).
    ToTrading,
    /// Перевода с этого счёта на торговый в адаптере нет (фьючерсы, earn, сводные строки и т.п.).
    Other,
}

/// Результат кнопки «Проверить подключение».
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionCheck {
    /// true — «Подключено».
    pub ok: bool,
    /// Точная причина сбоя или краткое описание успеха (например, права ключа).
    pub details: String,
}

/// Вид пула, который умеет адаптер DEX: id области настроек «DEX» и что это.
/// Нужен интерфейсу, чтобы показать настройки dex.* по каждой DEX и виду пула.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DexFamily {
    /// Имя DEX. С решения 58 каждая DEX индексатора — своя строка таблицы сопоставления: dex = family = id DEX у
    /// индексатора (например «pancakeswap-v3-bsc»).
    pub dex: String,
    /// id области «DEX» для dex.kind, dex.pool_enabled и адресов контрактов (id DEX у индексатора).
    pub family: String,
    /// Вид пулов (dex.kind). None — вид не задан или не поддерживается (решение 128): id из справочника пулов или с
    /// уже заданными настройками — чтобы в интерфейсе можно было задать его dex.kind и адреса.
    pub kind: Option<PoolKind>,
    /// Название для интерфейса.
    pub title: String,
    /// Ссылки на официальную документацию (адреса контрактов).
    pub docs: Vec<String>,
}

/// Контракт адаптера. Все функции возвращают данные в формате ядра.
/// Ошибки площадки адаптер переводит в коды gateway/errors.rs.
/// Функция, которая к площадке не применима, честно отвечает ошибкой NotApplicable —
/// никаких выдуманных данных (CLAUDE.md, «Никаких моков и заглушек»).
#[async_trait]
pub trait Venue: Send + Sync {
    /// Идентификатор площадки. Не может ошибиться.
    fn id(&self) -> VenueId;

    /// Вид площадки. Не может ошибиться.
    fn kind(&self) -> VenueKind;

    /// Проверка подключения для кнопки в интерфейсе: ключи, права, доступность.
    /// Возвращает: ConnectionCheck. Ошибка — только если проверку невозможно выполнить.
    async fn check_connection(&self) -> GatewayResult<ConnectionCheck>;

    /// Справочник: монеты площадки и их сети (ввод/вывод, комиссия вывода, адрес контракта).
    /// Возвращает: список по всем монетам и сетям. Ошибки: связь, ключ, разбор ответа.
    async fn coin_networks(&self) -> GatewayResult<Vec<CoinNetworkInfo>>;

    /// Справочник одной монеты (решение 113): предпроверке сделки нужен свежий статус ввода/вывода одной монеты, а не
    /// всех монет биржи. По умолчанию — coin_networks() с фильтром по тикеру (у API нет запроса одной монеты — так
    /// у MEXC). Адаптер с таким запросом в официальной документации (BingX: config/getall?coin=) переопределяет.
    /// Принимает: тикер. Возвращает: сети монеты (пусто — монеты нет). Ошибки: как coin_networks.
    async fn coin_networks_of(&self, ticker: &str) -> GatewayResult<Vec<CoinNetworkInfo>> {
        Ok(self
            .coin_networks()
            .await?
            .into_iter()
            .filter(|i| i.asset.ticker == ticker)
            .collect())
    }

    /// Подписка на рыночные данные (стаканы или пулы) по списку монет.
    /// Принимает: монеты и канал, куда адаптер шлёт события постоянно, пока жив канал.
    /// Возвращает: Ok после успешной подписки. Ошибки: связь, лимиты подписок.
    async fn subscribe_market(
        &self,
        assets: Vec<Asset>,
        out: mpsc::Sender<MarketEvent>,
    ) -> GatewayResult<()>;

    /// Котировка на объём по текущим данным (стакан по уровням, пул по содержимому).
    /// Принимает: монету, сторону, количество. Возвращает: Quote. Ошибки: нет данных, данные устарели.
    async fn quote(&self, asset: &Asset, side: Side, qty: Decimal) -> GatewayResult<Quote>;

    /// Решение 217: снимок данных котировки стороны на этот момент (gateway/frozen.rs) — у биржи копия её стакана, у
    /// DEX состояния пулов. Только память адаптера, без запросов к площадке: ядро берёт снимок на пути сделки (до
    /// ордера покупки), а котировки по нему считает фоном. Принимает: монету, сторону.
    /// Ошибки: те же, что у котировки до расчёта (нет данных, устарели, не задана настройка); NotApplicable — снимка
    /// у площадки нет (ядро пишет «не посчитано: нет снимка», а не считает по живым данным).
    async fn quote_snapshot(
        &self,
        _asset: &Asset,
        _side: Side,
    ) -> GatewayResult<super::frozen::QuoteSnapshot> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!(
                "{}: снимок котировки площадкой не поддерживается",
                self.id().0
            ),
        ))
    }

    /// Решение 237 (Тигран 09.11, живой случай XTER 08.11: стакан MEXC 32 с, котировка прошла, ордер отказал «стакан
    /// устарел» и 3 пустых повтора): снимок стакана пары ПО REST — один публичный запрос стакана по официальной
    /// документации биржи (ссылка на раздел и лимит — в комментарии функции адаптера), комиссия — тот же план, что у
    /// quote_snapshot (одна формула, gateway/frozen.rs FrozenBook). Зовёт ядро (core/exec_book.rs), только на пути
    /// ордера и только если quote_snapshot ответил StaleData (кадр пары старше venue.book_stale_ms без подтверждения
    /// тихой пары, стакана нет, соединение молчит). В память адаптера снимок не обязан попадать: котировка и худшая цена
    /// ордера берут его из ядра (один снимок на оба шага). Принимает: монету (тикер площадки), сторону.
    /// Ошибки: NotApplicable — у площадки нет (DEX, по умолчанию); NotConfigured; связь, RateLimited, коды биржи;
    /// BadResponse — вид ответа; комиссия — как у quote_snapshot.
    async fn quote_rest(
        &self,
        _asset: &Asset,
        _side: Side,
    ) -> GatewayResult<super::frozen::QuoteSnapshot> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            None,
            format!(
                "{}: снимок стакана по REST площадкой не поддерживается",
                self.id().0
            ),
        ))
    }

    /// Покупка или продажа: ордер на бирже или своп на DEX.
    /// Возвращает: фактический результат. Ошибки: отклонено, не исполнено, баланс, сеть.
    async fn trade(&self, req: TradeRequest) -> GatewayResult<TradeResult>;

    /// Проверка ордера без исполнения (решение 134): примет ли площадка ордер `req` (сторона, монета, количество,
    /// худшая цена) — по ТЕМ ЖЕ правилам, что адаптер применяет к настоящему ордеру (одна функция у обоих), и пробным
    /// ордером площадки, если он есть и включён настройкой. Ядро зовёт перед покупкой для стороны продажи биржи B.
    /// Возвращает: Ok — ордер пройдёт правила. Ошибки: точная причина (OrderRejected, NotConfigured, связь и др.);
    /// по умолчанию NotApplicable — площадка проверку не умеет (ядро считает это отказом: не торгуем вслепую).
    async fn check_order(&self, req: &TradeRequest) -> GatewayResult<()> {
        let _ = req;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "площадка не умеет проверять ордер без исполнения",
        ))
    }

    /// Решение 234: симуляция продажи DEX ДО покупки — продажа `req` (сторона Sell, количество — ожидаемый приход, худшая
    /// цена и маршрут — как у настоящей продажи) собирается тем путём исполнения, которым пойдёт настоящая, кошельком
    /// этого вида площадки (вид на счёт сделки, for_account) и прогоняется симуляцией; ничего не отправляется.
    /// Возвращает: выход после реального налога токена (ядро пересчитывает выгодность по нему).
    /// Ошибки: откат симуляции или отказ сборки — причина (NoRoute, QuoteMismatch, …); NotApplicable — площадка
    /// симуляцию не умеет или эту продажу без денег на кошельке не симулировать (причина в тексте).
    async fn dex_presell(
        &self,
        req: &TradeRequest,
    ) -> GatewayResult<crate::core::types::DexPresell> {
        let _ = req;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет симуляции продажи до покупки",
        ))
    }

    /// Вывод монеты с площадки на адрес. Для DEX — перевод из своего кошелька.
    /// Возвращает: созданный перевод. Ошибки: адрес, права, баланс, вывод закрыт.
    async fn withdraw(&self, req: WithdrawRequest) -> GatewayResult<Transfer>;

    /// Текущий статус вывода по идентификатору площадки.
    /// Возвращает: перевод с актуальным статусом. Ошибки: не найден, связь.
    async fn withdrawal_status(&self, venue_ref: &str) -> GatewayResult<Transfer>;

    /// Решение 231: окончательный итог ордера (свопа), чей исход был неизвестен, — по «чем найти» из ошибки
    /// (gateway/pending.rs), официальным запросом ордера площадки. Найден не окончательным — адаптер доводит его как
    /// обычный (ждёт, отменяет остаток — правило Тиграна). Принимает: исходный запрос, PendingRef.
    /// Возвращает: Some(итог) — как у trade (исполнено / частично / не исполнено, суммы, комиссии); None — пока не найден
    /// (ядро спросит ещё). Ошибки: сбой запроса (ядро спросит ещё); по умолчанию NotApplicable — однозначного способа
    /// нет, исход выясняет человек.
    async fn resolve_trade(
        &self,
        req: TradeRequest,
        pending: &super::pending::PendingRef,
    ) -> GatewayResult<Option<TradeResult>> {
        let _ = (req, pending);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "площадка не умеет искать ордер по своему ID — исход проверяет человек",
        ))
    }

    /// Решение 231: создан ли вывод, чей исход был неизвестен, — по своему ID вывода (история выводов площадки) или
    /// по хешу транзакции (DEX). Принимает: исходный запрос, PendingRef. Возвращает: Some(перевод с ID площадки) —
    /// дальше ядро ведёт его обычным опросом статуса; None — пока не найден. Ошибки: сбой запроса (ядро спросит ещё);
    /// по умолчанию NotApplicable — однозначного способа нет, исход выясняет человек.
    async fn resolve_withdraw(
        &self,
        req: WithdrawRequest,
        pending: &super::pending::PendingRef,
    ) -> GatewayResult<Option<Transfer>> {
        let _ = (req, pending);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "площадка не умеет искать вывод по своему ID — исход проверяет человек",
        ))
    }

    /// Адрес для депозита монеты в указанной сети.
    /// Возвращает: адрес. Ошибки: ввод закрыт, сеть не поддерживается.
    async fn deposit_address(&self, asset: &Asset, network: &str) -> GatewayResult<String>;

    /// Статус депозита по хешу транзакции в сети.
    /// Возвращает: перевод с актуальным статусом. Ошибки: не найден, связь.
    async fn deposit_status(&self, asset: &Asset, tx_hash: &str) -> GatewayResult<Transfer>;

    /// Балансы всех активов на площадке (или в кошельке для DEX).
    /// Ошибки: ключ, связь.
    async fn balances(&self) -> GatewayResult<Vec<Balance>>;

    /// Решение 144 (Тигран, 04.11): балансы ВСЕХ счетов площадки — спот, фьючерсы, фонд/funding, earn и др. — для панели
    /// «Балансы» интерфейса. Только чтение, на торговлю не влияет. Ошибка одного счёта — в его AccountBalances::error,
    /// остальные счета показываются. По умолчанию — один счёт «Спот» из balances().
    /// Возвращает: счета со строками. Ошибки: как balances (только если не прочитан ни один счёт).
    async fn account_balances(&self) -> GatewayResult<Vec<AccountBalances>> {
        let b = self.balances().await?;
        Ok(vec![AccountBalances {
            account: "Спот".into(),
            balances: b
                .into_iter()
                .filter(|x| !x.free.is_zero() || !x.locked.is_zero())
                .collect(),
            error: None,
            note: None,
        }])
    }

    /// Решение 174 (Тигран, 05.11.2026): роль счёта `account` (название — как его отдаёт account_balances) для кнопки
    /// «Продать» панели «Балансы»: торговый, перекладываемый на торговый методом deposit_to_trading, или ни тот, ни
    /// другой. По умолчанию — «Спот» торговый, прочие — Other (перевода нет). Адаптер с другим названием торгового счёта
    /// или с переводом фонд → торговый переопределяет. Ошибок нет.
    fn account_role(&self, account: &str) -> AccountRole {
        if account == "Спот" {
            AccountRole::Trading
        } else {
            AccountRole::Other
        }
    }

    /// Баланс одного актива (решение 113): на пути сделки нужна одна монета (BNB на газ, USDT перед покупкой), а не
    /// весь список. По умолчанию — balances() с фильтром (биржа: один REST и так отдаёт все активы). DEX переопределяет:
    /// один запрос к узлу вместо запроса по каждому токену. Строка — та же, что дал бы balances().
    /// Принимает: тикер. Возвращает: строку баланса; None — актива нет. Ошибки: как balances.
    async fn balance(&self, asset: &str) -> GatewayResult<Option<Balance>> {
        Ok(self
            .balances()
            .await?
            .into_iter()
            .find(|b| b.asset == asset))
    }

    /// Решение 150 (Тигран, 04.11.2026): что биржа сейчас держит от вывода, хотя на балансе это свободно (MEXC —
    /// предварительно зачисленные депозиты: торговать можно, выводить нельзя до полного числа подтверждений). Ядро
    /// вызывает перед покупкой с выводом (core/precheck_withdraw.rs).
    /// Решение 166 (Тигран, 05.11.2026): у ВСЕХ бирж — из официального источника площадки (удержания депозитов,
    /// квота вывода, «можно вывести» по счёту). По умолчанию — «неизвестно» без запросов (WithdrawHold::unknown): у
    /// площадки такого способа нет или он не описан в её документации; это НЕ «удержаний нет».
    /// Принимает: котируемую монету покупки (venue.quote_asset, например USDT) — для источников по одной монете.
    /// Возвращает: список удержаний (монета, сколько, почему) или «неизвестно». Ошибки: ключ, связь, разбор ответа.
    async fn withdraw_hold(&self, _quote: &str) -> GatewayResult<crate::core::types::WithdrawHold> {
        Ok(crate::core::types::WithdrawHold::unknown(
            "в официальной документации площадки нет способа узнать, сколько можно вывести сейчас",
        ))
    }

    /// Решение 175 (Тигран, 05.11.2026): пары биржи в делистинге — тикер, символ, когда прекращаются торги и (если
    /// площадка сообщает) закрывается вывод, источник в API. Ядро опрашивает фоном (core/delist.rs, delist.refresh_s) и
    /// не торгует монету, если на любой стороне сделки она в делистинге и до него меньше delist.block_hours.
    /// По умолчанию — «неизвестно» без запросов (Delistings::unknown): у площадки нет официального источника или он не
    /// описан в её документации; это НЕ «делистингов нет». HTML-страницы объявлений не разбираются. Решение 226:
    /// метка риска делистинга пары (ST) и заголовки анонсов из API биржи (gateway/delist_ann.rs) — записи без времени;
    /// сбой второстепенного источника — Delistings::warnings, основной действует.
    /// Возвращает: список пар в делистинге или «неизвестно». Ошибки: связь, ключ, разбор ответа.
    async fn delistings(&self) -> GatewayResult<crate::core::types::Delistings> {
        Ok(crate::core::types::Delistings::unknown(
            "в официальной документации площадки нет признака или времени делистинга пар в API",
        ))
    }

    /// Сколько стоит вывести монету с площадки: биржа — комиссия вывода из её API;
    /// DEX — газ перевода из кошелька (BNB) и налог токена на перевод.
    /// Принимает: монету, сеть, сумму. Возвращает: список затрат (Cost).
    /// Ошибки: нет данных, сеть не найдена, монета не проверена (DEX: CoinNotChecked).
    async fn withdraw_cost(
        &self,
        asset: &Asset,
        network: &str,
        amount: Decimal,
    ) -> GatewayResult<Vec<Cost>>;

    /// Перепроверить налоги монет и крючки их пулов сейчас, не дожидаясь цикла (кнопка в интерфейсе).
    /// Результаты приходят событиями CoinCheck / PoolCheck в канал subscribe_market.
    /// Ошибки: NotApplicable — у площадки нет такой проверки (биржа).
    async fn recheck_coins(&self, assets: Vec<Asset>) -> GatewayResult<()>;

    /// Виды пулов, которые умеет адаптер DEX (для настроек dex.* в интерфейсе).
    /// Ошибки: NotApplicable — не DEX.
    async fn dex_families(&self) -> GatewayResult<Vec<DexFamily>>;

    /// Каналы отправки транзакций, которые умеет адаптер (решение 41).
    /// Ошибки: NotApplicable — у площадки нет каналов отправки (биржа).
    async fn send_channels(&self) -> GatewayResult<Vec<SendChannelInfo>>;

    /// «Проверить подключение» одного канала отправки: адрес, ключ, сеть.
    /// Возвращает: ConnectionCheck. Ошибки: NotApplicable (биржа); неизвестный канал — NotConfigured.
    async fn check_send_channel(&self, id: &str) -> GatewayResult<ConnectionCheck>;

    /// Решение 132: вид площадки, привязанный к счёту (кошельку) DEX — им идут сделка, вывод, адрес депозита, балансы,
    /// очередь, исполнитель этого кошелька. Без привязки DEX — только чтение и симуляции (кошелёк строки «пробы»).
    /// Принимает: id счёта. Ошибки: NotApplicable — не DEX (биржа: счёт ей не нужен); NotConfigured — нет такого кошелька.
    fn for_account(
        &self,
        account: &crate::core::types::DexAccountId,
    ) -> GatewayResult<std::sync::Arc<dyn Venue>> {
        let _ = account;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет счетов DEX",
        ))
    }

    /// Решение 132: адрес кошелька счёта (выводится из ключа у адаптера; ключ наружу не отдаётся). Только у вида DEX на
    /// счёт. Ошибки: NotApplicable — биржа; NotConfigured — счёт не выбран или ключ не введён; SecretsLocked.
    async fn wallet_address(&self) -> GatewayResult<String> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет кошелька",
        ))
    }

    /// Очередь транзакций кошелька (DEX, решение 45): свободна / ждёт итога / остановлена до ручной проверки.
    /// Ошибки: NotApplicable — у площадки нет кошелька (биржа).
    async fn wallet_queue(&self) -> GatewayResult<WalletQueue> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет очереди кошелька",
        ))
    }

    /// «Снять очередь после ручной проверки»: очередь, остановленная с неизвестным исходом, снова свободна.
    /// Принимает: кто снимает (для журнала); подтверждено ли снятие при ещё не занятом nonce (висящая транзакция
    /// может пройти позже). Ошибки: NotApplicable (биржа; очередь не остановлена); NonceConflict — nonce ещё не занят,
    /// а подтверждения нет.
    async fn release_wallet_queue(&self, by: &str, pending_ok: bool) -> GatewayResult<WalletQueue> {
        let _ = (by, pending_ok);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет очереди кошелька",
        ))
    }

    /// Справочник пулов от индексатора (решение 58): DEX индексатора, пулы монет и что с каждым сделал адаптер
    /// (взят, DEX не подключена, проверка на узле не прошла). Ошибки: NotApplicable — не DEX.
    async fn pool_directory(&self) -> GatewayResult<PoolDirectory> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет справочника пулов",
        ))
    }

    /// Пулы DEX глазами адаптера (решение 73): глубина по направлениям в полосе цены, спит ли пул, сверка с квотером,
    /// tickSpacing — для экрана «Монеты». Ошибки: NotApplicable — не DEX.
    async fn pool_depths(&self) -> GatewayResult<Vec<crate::core::types::PoolDepth>> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет пулов",
        ))
    }

    /// Только перечисленные пулы (решение 74: пулы маршрута в карточке сигнала — без расчёта всего реестра).
    /// По умолчанию — pool_depths с фильтром. Ошибки: как pool_depths.
    async fn pool_depths_of(
        &self,
        pools: &[crate::core::types::PoolRef],
    ) -> GatewayResult<Vec<crate::core::types::PoolDepth>> {
        let all = self.pool_depths().await?;
        Ok(all
            .into_iter()
            .filter(|d| pools.contains(&d.pool))
            .collect())
    }

    /// Сигналы циклов DEX ↔ DEX (решение 85, фаза сигнала): замкнутые циклы базовых монет с чистой прибылью, маршрутом
    /// и флагом исполнимости — для «Живого экрана» и ядра. Направление за dex_dex.enabled; выключено — пусто. Расчёт сам
    /// сделок не запускает; по ним ядро ведёт сделку цикла (решение 124, core/cycle_watch.rs).
    /// Ошибки: NotApplicable — не DEX; прочие — как у котировки (узел, настройки).
    async fn dex_cycles(&self) -> GatewayResult<Vec<crate::core::types::DexCycle>> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет циклов DEX ↔ DEX",
        ))
    }

    /// «Сухая сделка DEX» (решение 121, C4): маршрут и транзакция собираются своим сборщиком КАК ДЛЯ СДЕЛКИ и
    /// прогоняются eth_simulateV1 от кошелька бота — ничего не отправляется, nonce и очередь кошелька не заняты.
    /// Принимает: монету, сторону, сумму USDT. Возвращает: итог (форма маршрута, выходы, газ, минимум, причина отказа).
    /// Ошибки: NotApplicable — не DEX; остальное (нет маршрута, узел) — причиной в итоге.
    async fn dex_dry_swap(
        &self,
        asset: Asset,
        side: Side,
        usdt: Decimal,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let _ = (asset, side, usdt);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет сухой сделки DEX",
        ))
    }

    /// «Сухая сделка DEX» через свой исполнитель ArbExecutor с подставленным в симуляцию кодом (решение 122, часть X):
    /// любой маршрут собирается шагами исполнителя и прогоняется eth_simulateV1 с подменой состояния — проверка
    /// исполнителя до развёртывания. Ничего не отправляет. Ошибки: NotApplicable — не DEX; остальное — причиной в итоге.
    async fn dex_dry_swap_executor(
        &self,
        asset: Asset,
        side: Side,
        usdt: Decimal,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let _ = (asset, side, usdt);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет исполнителя DEX",
        ))
    }

    /// Расчёт цикла DEX ↔ DEX на вход `input` базовой монеты (решение 124): по состоянию пулов в памяти адаптера — выход,
    /// затраты (газ свопов, разрешения, чаевые) в BNB и в базовой, чистая прибыль, курсы базовой и BNB в USDT, форма
    /// исполнения; with_wallet — ещё свободный баланс базовой у кошелька (запрос к узлу). Ядро зовёт его на каждую точку
    /// подбора размера (core/size_search.rs). Ошибки: NotApplicable — не DEX; NoRoute — цикл не исполним (причина) или
    /// шаг не исполним целиком (объём велик); NotConfigured, StaleData, узел.
    async fn dex_cycle_quote(
        &self,
        cycle: &crate::core::types::DexCycle,
        input: Decimal,
        with_wallet: bool,
    ) -> GatewayResult<crate::core::types::CycleQuote> {
        let _ = (cycle, input, with_wallet);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет циклов DEX ↔ DEX",
        ))
    }

    /// Сделка цикла DEX ↔ DEX (решение 124): пулы перечитываются на свежем блоке, цикл собирается исполнителем
    /// ArbExecutor (одна транзакция) или бандлом свопов (решение 84), симулируется и отправляется ОДИН раз — без повторов.
    /// Ошибки: NotApplicable — не DEX; NoRoute / QuoteMismatch / StaleData — до отправки (сделки не было);
    /// SwapReverted / TxNotIncluded / OutcomeUnknown — с фактом транзакции.
    async fn dex_cycle_trade(
        &self,
        req: crate::core::types::CycleTradeRequest,
    ) -> GatewayResult<crate::core::types::CycleTradeResult> {
        let _ = req;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет циклов DEX ↔ DEX",
        ))
    }

    /// Решение 233 (дополнение, п. 4): окончательный итог сделки цикла, чей исход был неизвестен, — по «чем найти» из
    /// ошибки dex_cycle_trade (хеш(и), nonce, контекст разбора — собственность адаптера). Принимает: исходный запрос,
    /// PendingRef. Возвращает: Some(итог — как у dex_cycle_trade; «не исполнено окончательно» — нули, статус NotFilled,
    /// транзакция NotIncluded); None — пока не установлен (ядро спросит ещё). Ошибки: SwapReverted — откатился в блоке
    /// (окончательно); сбой запроса (ядро спросит ещё); по умолчанию NotApplicable — исход выясняет человек.
    async fn resolve_cycle(
        &self,
        req: crate::core::types::CycleTradeRequest,
        pending: &super::pending::PendingRef,
    ) -> GatewayResult<Option<crate::core::types::CycleTradeResult>> {
        let _ = (req, pending);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет циклов DEX ↔ DEX — исход проверяет человек",
        ))
    }

    /// «Сухая сделка» цикла (решение 124): та же сборка, что dex_cycle_trade, и eth_simulateV1 от кошелька — ничего не
    /// отправляется; executor = true — через исполнитель с подставленным кодом (решение 122). Ошибки: NotApplicable —
    /// не DEX; остальное — причиной в итоге.
    async fn dex_dry_cycle(
        &self,
        req: crate::core::types::CycleTradeRequest,
        executor: bool,
    ) -> GatewayResult<crate::core::types::DexDrySwap> {
        let _ = (req, executor);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет циклов DEX ↔ DEX",
        ))
    }

    /// Оценка развёртывания своего исполнителя ArbExecutor (решение 122, часть X): газ, цена, стоимость в BNB и USDT,
    /// будущий адрес, хеш байткода. Ничего не отправляет. Ошибки: NotApplicable — не DEX; ключ, каналы, узел.
    async fn dex_executor_quote(&self) -> GatewayResult<crate::core::types::ExecutorDeployQuote> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет исполнителя DEX",
        ))
    }

    /// Развёртывание своего исполнителя ArbExecutor с кошелька бота (решение 122, часть X) — действие с деньгами, только
    /// по подтверждению человека. Принимает: SHA-256 байткода из оценки, которую видел человек. Ошибки: NotApplicable —
    /// не DEX; QuoteMismatch — байткод не тот; ключ, каналы, узел.
    async fn dex_executor_deploy(
        &self,
        sha256: String,
    ) -> GatewayResult<crate::core::types::ExecutorDeployed> {
        let _ = sha256;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет исполнителя DEX",
        ))
    }

    /// «Проверить подключение» своего исполнителя ArbExecutor (ревью 122): код по exec.executor_address совпал с
    /// репозиторием, владелец — кошелёк бота. Возвращает: ConnectionCheck. Ошибки: NotApplicable — не DEX.
    async fn check_executor(&self) -> GatewayResult<ConnectionCheck> {
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет исполнителя DEX",
        ))
    }

    /// Ядро больше не ждёт депозит монеты на эту площадку (вывод на её адрес не состоялся или прерван):
    /// площадка снимает ожидание поступления. Ошибок нет у тех, кому это не нужно (биржа — ничего не делает).
    async fn deposit_abandoned(&self, asset: &Asset) -> GatewayResult<()> {
        let _ = asset;
        Ok(())
    }

    /// Решение 174: переложить монету со счёта роли ToTrading (account_role: фонд / main / Funding) на торговый — по
    /// явному действию человека (кнопка «Продать» панели «Балансы», «Продать сейчас» / «Продать и перегнать» по монете,
    /// лежащей не на торговом счёте). Тот же запрос перевода, что у deposit_to_trading, но без его условий (настройка
    /// venue.deposit_to_spot, вид адреса депозита): человек сам указал, что монета там. Принимает: монету, сумму.
    /// Возвращает: текст перевода от площадки. Ошибки: отказ площадки, OutcomeUnknown; по умолчанию NotApplicable —
    /// у площадки перевода на торговый счёт нет.
    async fn move_to_trading(&self, asset: &Asset, amount: Decimal) -> GatewayResult<String> {
        let _ = (asset, amount);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет перевода на торговый счёт",
        ))
    }

    /// Зачисленный перевод переложить туда, где площадка торгует (решение 102: у BingX депозит приходит на фондовый
    /// счёт, торговля — на споте). Принимает: монету, сумму зачисления. Возвращает: None — перекладывать не нужно
    /// (или выключено настройкой); Some(текст) — переложено. Ошибки: отказ площадки. По умолчанию — ничего не нужно.
    async fn deposit_to_trading(
        &self,
        asset: &Asset,
        amount: Decimal,
    ) -> GatewayResult<Option<String>> {
        let _ = (asset, amount);
        Ok(None)
    }

    /// Запись окна (решение 125): начать писать сырые кадры стакана монеты и подписаться на её канал сделок (своё
    /// соединение на время окна) — в приёмник `sink` (gateway/tap.rs). Принимает: монету, приёмник. Возвращает:
    /// каналы и вид ленты сделок. Ошибки: NotApplicable — у площадки нет стакана (DEX); NotConfigured.
    async fn window_tap(
        &self,
        asset: &Asset,
        sink: super::tap::TapSink,
    ) -> GatewayResult<super::tap::TapInfo> {
        let _ = (asset, sink);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет стакана и ленты сделок",
        ))
    }

    /// Запись окна закончена: перестать писать кадры монеты, отписаться от сделок и закрыть их соединение.
    /// Ошибки: NotApplicable — как window_tap.
    async fn window_untap(&self, asset: &Asset) -> GatewayResult<()> {
        let _ = asset;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет стакана и ленты сделок",
        ))
    }

    /// Разбор сырого кадра окна для показа (стакан или сделки в формате ядра). Принимает: поток, содержимое.
    /// Ошибки: NotApplicable — как window_tap; BadResponse — кадр не разобран (причина).
    fn decode_tap(
        &self,
        stream: super::tap::TapStream,
        data: &super::tap::TapData,
    ) -> GatewayResult<crate::core::types::TapDecoded> {
        let _ = (stream, data);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "у площадки нет стакана и ленты сделок",
        ))
    }

    /// Решение 202: свежесть потока стаканов по соединениям (пачкам пар) для сторожа ядра (core/feed_watch.rs).
    /// None — адаптер сторожа не поддерживает (сторож для площадки выключен). Ошибок нет.
    fn feed_health(&self) -> Option<super::feed_health::FeedHealth> {
        None
    }

    /// Решение 202: переподписать одну пару соединения (отписка + подписка), соседей не трогая.
    /// Принимает: имя соединения, тикер ядра. Ошибки: NotApplicable — не поддерживается; BadResponse — команда не
    /// доставлена (соединение остановлено).
    async fn resubscribe_pair(&self, conn: &str, ticker: &str) -> GatewayResult<()> {
        let _ = (conn, ticker);
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "переподписка одной пары не поддерживается",
        ))
    }

    /// Решение 202: переподключить одно соединение с теми же парами (другие соединения не трогаются).
    /// Принимает: имя соединения. Ошибки: NotApplicable — не поддерживается; BadResponse — команда не доставлена.
    async fn reconnect_conn(&self, conn: &str) -> GatewayResult<()> {
        let _ = conn;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "переподключение одного соединения не поддерживается",
        ))
    }

    /// Решение 213 (Тигран, 07.11.2026: «тишина пары ≠ смерть»): версия стакана пары на бирже по REST — для сторожа
    /// ядра (core/feed_watch.rs), когда пара молчит при живом соединении. Ядро сравнивает её с версией последнего
    /// кадра пары (PairHealth.last_id / last_changed_ms из feed_health) по правилам core/feed_confirm.rs::verdict:
    /// книга не менялась — пара «тихая, подтверждена» (не переподписывать, book_confirmed); новее — кадры потеряны,
    /// переподписка (решение 202).
    ///
    /// Как реализовать адаптеру (12 бирж — один контракт):
    /// - ОДИН публичный REST-запрос стакана одной пары по официальной документации биржи (ссылка на раздел и на лимит
    ///   частоты публичных запросов — в комментарии функции адаптера); глубина — наименьшая, какую документация
    ///   разрешает без выдумок (или по умолчанию), уровни не нужны — только версия;
    /// - заполнить BookVersion тем, что даёт биржа: `id` — только если по документации это тот же счётчик, что номер
    ///   книги в кадрах WebSocket, который адаптер передаёт в ConnHandle::frame_ver (Gate: id ↔ lastUpdateId; Binance:
    ///   lastUpdateId ↔ lastUpdateId); `changed_ms` — время последнего изменения книги по часам биржи; `server_ms` —
    ///   время ответа по часам биржи. Поля, которых у биржи нет, — None (не подставлять время бота!);
    /// - на каждом кадре стакана вызывать ConnHandle::frame_ver с номером/временем из кадра той же шкалы, и отдавать
    ///   feed_health с confirm = true; реализовать book_confirmed (ниже);
    /// - запрос идёт через общий REST адаптера: паузу после 429 он соблюдает сам; темп задаёт ядро
    ///   (venue.quiet_confirm_max_per_s, venue.quiet_confirm_min_ms).
    ///
    /// Второй способ — ПО СОДЕРЖИМОМУ (Тигран 08.11), если документация биржи не связывает номер кадра WebSocket с
    /// номером ответа REST: запросить у REST столько уровней, сколько даёт кадр стакана (venue.depth_levels), и
    /// заполнить BookVersion.print — первые N уровней ответа и первые N уровней своего стакана в памяти (того, по
    /// которому считается котировка), взятого ПОСЛЕ ответа (BookPrint::top); `id` / `changed_ms` — None. Ядро
    /// сравнивает точно (core/feed_print.rs): равны — тихая, подтверждена; не равны или стакана нет — кадры потеряны.
    /// ConnHandle::frame без версии, feed_health с confirm = true, book_confirmed — как выше. MEXC, Bitget.
    ///
    /// Принимает: тикер ПЛОЩАДКИ (решение 211 — тот, под которым пара подписана и стоит в PairHealth.ticker).
    /// Возвращает: версию стакана. Ошибки: NotApplicable — адаптер не умеет (по умолчанию); NotConfigured — настройка
    /// адаптера не задана; связь, RateLimited, коды биржи — как у любого REST.
    async fn book_version(&self, ticker: &str) -> GatewayResult<super::feed_health::BookVersion> {
        let _ = ticker;
        Err(GatewayError::new(
            ErrorCode::NotApplicable,
            Some(self.id()),
            "версия стакана по REST не поддерживается",
        ))
    }

    /// Решение 213: ядро подтвердило по REST, что книга пары на бирже не менялась с последнего кадра (или с подписки),
    /// — стакан этой пары в котировке адаптера (quote) считается свежим до `until_ms` (unix мс, часы бота), даже если
    /// кадра нет дольше venue.book_stale_ms. Правило адаптера: кадр не старше venue.book_stale_ms ИЛИ сейчас не позже
    /// until_ms; проверку живости соединения (feed.stale_after_ms) подтверждение НЕ отменяет. Подтверждение относится к
    /// стакану, который сейчас в памяти: новый кадр с другой книгой или снятие стакана (сбой подписки) его снимает.
    /// Принимает: тикер площадки, срок. Ошибок нет; по умолчанию ничего не делает (адаптер без confirm).
    fn book_confirmed(&self, ticker: &str, until_ms: i64) {
        let _ = (ticker, until_ms);
    }
}
