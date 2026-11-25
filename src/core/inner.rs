//! Файл: core/inner.rs
//! Что делает: общее состояние ядра — рынок, справочник, чёрный список, сигналы, очередь расчёта,
//! сделки, риск, ручное управление, статистика газа — и его создание.
//! Кто вызывает: core/engine.rs (создаёт), все модули ядра (читают и меняют под короткими блокировками).
//! От чего зависит: модули ядра (их типы), core/types, gateway.
//! Настройки: нет.
//!
//! Блокировки — std::sync и никогда не держатся через await: тик не ждёт других задач.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, RwLock};

use alloy_primitives::Address;
use rust_decimal::Decimal;
use tokio::sync::{Notify, broadcast, mpsc};

use crate::core::types::{Asset, CoinGasStats, Direction, ManualHolding, StuckAlert, VenueId};
use crate::gateway::contract::VenueKind;
use crate::gateway::events::MarketEvent;

use super::blacklist::Blacklist;
use super::deal::Deal;
use super::deps::Deps;
use super::limits::Exposure;
use super::market_state::MarketState;
use super::risk::Risk;
use super::signal::{Signals, SpreadView};
use super::view::Live;
use super::vol::Volatility;

/// Ключ истории времени в пути: монета, A, B.
pub type TransitKey = (Asset, VenueId, VenueId);

/// Общее состояние ядра.
pub struct Inner {
    pub deps: Deps,
    pub settlement: super::settlement_unit::Book,
    pub live: Live,
    pub market: RwLock<MarketState>,
    /// Справочник сетей каждой биржи (последний успешный coin_networks).
    pub networks: RwLock<super::registry_build::Networks>,
    /// Решение 266 (ревью): смены статуса ввода/вывода, ждущие подтверждения сглаживателя (status_debounce.rs), по
    /// биржам — пока смена монеты ждёт, предпроверка справочнику сетей из памяти не верит (precheck_nets.rs).
    pub status_pending: RwLock<super::status_debounce::Pending>,
    pub withdraw_fee_refresh: Mutex<HashSet<(VenueId, String, String)>>,
    /// Площадки реестра шлюза и их вид (заполняется при старте).
    pub kinds: RwLock<Vec<(VenueId, VenueKind)>>,
    /// Справочник монет и направлений.
    /// Решение 131: снимок за Arc, рабочие и неподвижные монеты (coin_book.rs); разыменование — рабочие.
    pub coins: RwLock<super::coin_book::CoinBook>,
    /// (площадка, тикер) → монета для тика стакана (решение 118, Я-9, ticker_index.rs).
    pub tickers: RwLock<super::ticker_index::TickerIndex>,
    /// Адрес USDT сети по умолчанию (решение 430; BSC) или причина, почему его нет.
    pub usdt: RwLock<Result<Address, String>>,
    /// Решение 430: адреса USDT сетей не по умолчанию (registry_chains.rs); адрес сети по умолчанию — `usdt`.
    pub usdt_other: RwLock<HashMap<crate::core::types::ChainId, Address>>,
    pub blacklist: RwLock<Blacklist>,
    /// Пулы, исключённые по крючку (чтобы уведомлять только о новых).
    pub excluded_pools: Mutex<HashSet<(VenueId, String)>>,
    /// Последнее сообщённое (журнал, Telegram) состояние чёрного списка монеты: вид причины; нет записи — «не в списке».
    /// «Ещё не проверена» (NotChecked — в том числе временно, пока пулы недостоверны после обрыва ленты) его не меняет:
    /// уведомления — только на настоящую смену (решение 71, rebuild_report.rs).
    pub told_blocks: Mutex<std::collections::HashMap<crate::core::types::Asset, String>>,
    /// Монеты DEX, чьи пулы сняты до перечитывания (Invalidated): решение по чёрному списку держится, пока пул снова не
    /// придёт (Touched::Pool). Пула нет по справочнику (NoUsdtPool без этой отметки) — решение снимается (решение 71).
    pub held: Mutex<HashSet<(VenueId, alloy_primitives::Address)>>,
    /// Сколько монет ждали проверки налогов при прошлой пересборке (для coin.check_done).
    pub not_checked: Mutex<usize>,
    /// Первый проход проверки налогов после запуска: пока он идёт, вход и выход из чёрного списка не уведомляются по
    /// монетам — копятся причины для одного сводного coin.check_done (core/first_pass.rs).
    pub first_pass: Mutex<super::first_pass::FirstPass>,
    pub signals: Mutex<Signals>,
    /// Волатильность цен по замерам сигнала (vol.rs).
    pub vol: Mutex<Volatility>,
    pub spreads: RwLock<HashMap<Direction, SpreadView>>,
    /// Подписки площадок: на что подписана, идёт ли подписка, причина сбоя (sub_state.rs).
    pub subs: Mutex<super::sub_state::SubState>,
    /// Решение 489: выключенные площадки DEX, чью сеть сейчас держат сделки (net_hold.rs) — один толчок на отпускании.
    pub net_held: Mutex<HashSet<VenueId>>,
    /// Решение 196: время последнего события рынка (стакан, пул) по площадке — живость потока (feed_live.rs).
    pub feed_seen: super::feed_live::Seen,
    /// Монеты, по которым идёт пересчёт тика, и площадки, приславшие тик во время пересчёта или сна (tick_plan.rs).
    /// Решение 109: повтор пересчитывает направления всех этих площадок, а не только первой; решение 322 — событие
    /// пула будит сон задачи монеты (signal.pool_event_now).
    pub ticking: Mutex<super::tick_plan::Plan>,
    /// Решение 318: сроки дозревания «держится» и спящие задачи таймера (signal_ripen.rs; signal.hold_timer).
    pub ripen: Mutex<super::signal_ripen::Plan>,
    /// Память котировок DEX для тика (решение 109, quote_cache.rs).
    pub quote_cache: Mutex<super::quote_cache::QuoteCache>,
    /// Решение 352: котировки стороны DEX сигнала по отпечатку входов (signal_dex.rs; signal.book_same_skip).
    pub signal_dex: super::signal_dex::Memo,
    /// Решение 349: пробуждение монет событием промежуточного пула — ждущие пары, отметки (mid_wake.rs).
    pub mid_wake: super::mid_wake::State,
    /// Последняя отправка строки «Живого экрана» по направлению: когда и с каким состоянием (решение 109).
    pub spread_sent: Mutex<HashMap<Direction, (std::time::Instant, super::signal::State)>>,
    /// Очередь тяжёлого расчёта (candidates.rs; с моментом постановки — замер очереди, решение 129) и направления,
    /// уже стоящие в ней.
    pub cand_tx: mpsc::Sender<(Direction, std::time::Instant)>,
    pub queued: Mutex<HashSet<Direction>>,
    /// Последний расчёт кандидата по направлению: {direction, verdict, at} — для «Живого экрана».
    pub candidates: Mutex<HashMap<Direction, serde_json::Value>>,
    /// Решение 155: открытая строка журнала отказа по направлению (core.candidate_rejected — одна строка на смену вида
    /// причины со счётчиком повторов; reject_log.rs).
    pub last_reject: super::reject_log::RejectLog,
    // Задача t165: память причины core.start_refused — в last_reject под ключом start_note::key (поля start_refused нет).
    /// Решение 148: защита по факту продажи выключена — причина, уже записанная в журнал (одна запись на смену).
    pub slip_off: Mutex<Option<String>>,
    /// История времени в пути: (монета, A, B) → длительности переводов по порядку, секунды (transit.rs).
    pub transits: Mutex<HashMap<TransitKey, Vec<Decimal>>>,
    /// Решение 464 (t238, ревью): готовые суммы переводов пар площадок по всем монетам (transit_source.rs).
    pub transit_pairs: super::transit_source::Pairs,
    /// Решение 464 (t238, ревью): ордера продажи в полёте, итоги продаж и сделки с неизвестным исходом продажи по
    /// (площадка, монета) — для расчёта новых сделок (sale_marks.rs).
    pub sales: super::sale_marks::Marks,
    /// Ожидаемые зачисления переводов и площадки, приславшие события баланса (credit_watch.rs, решение 76).
    pub credits: super::credit_watch::Credits,
    /// Деньги в открытых сделках (limits.rs). Ведёт executor.rs.
    pub exposure: Mutex<Exposure>,
    /// Канал событий адаптеров (передаётся в subscribe_market).
    pub market_tx: mpsc::Sender<MarketEvent>,
    /// «Толчок» обновления сетей бирж (аномальный спред — перечитать статусы ввода и вывода).
    pub kick_networks: Notify,
    /// Решение 489: поколение толчков справочника сетей (net_kick.rs) — толчок во время запроса справочника не теряется.
    pub networks_gen: std::sync::atomic::AtomicU64,
    /// «Толчок» пересборки справочника (несколько толчков подряд сливаются в одну пересборку).
    pub kick_rebuild: Notify,
    /// Тики монет — для режима «Ожидание» и ожидания котировки (stuck.rs).
    pub coin_ticks: broadcast::Sender<Asset>,
    /// Стоп-условия и режим системы (risk.rs).
    pub risk: Mutex<Risk>,
    /// Сделки по id (ledger.rs).
    pub deals: Mutex<HashMap<String, Deal>>,
    /// Монеты, по которым сейчас идёт сделка (решение 118, Я-2: одна сделка на монету — ключ по адресу контракта).
    pub active: Mutex<HashSet<super::coin_key::CoinKey>>,
    /// Решение 394 (t179): сделки монеты, которые уже купили и везут или продают монету (coin_ride.rs) — монета для
    /// них больше не в `active`; докупка (exec.top_up = да) — следующая сделка группы того же направления; выключена —
    /// одна сделка на монету, как раньше. Блокировки: сначала `active`, потом `rides` (coin_ride.rs).
    pub rides: Mutex<HashMap<super::coin_key::CoinKey, Vec<super::coin_ride::Ride>>>,
    /// Решение 394: группы докупки — отложенные части общего возврата USDT и закрывающиеся сделки (coin_group.rs).
    /// Блокировки: `groups` раньше `rides` и `deals`.
    pub groups: Mutex<HashMap<String, super::coin_group::Pool>>,
    /// Решение 394: продажи сделок групп докупки — по ним новый расчёт продажи той же монеты на той же площадке ждёт
    /// данных новее продажи (ride_trail.rs).
    pub trail: Mutex<HashMap<(super::coin_key::CoinKey, VenueId), super::ride_trail::Trail>>,
    /// Решение 394 (ревью 2 t179): сколько идущих сделок групп докупки и следов их продаж — 0: котировки и затраты на
    /// горячем пути не берут замков `rides` / `trail` (ride_quote::idle).
    pub group_live: std::sync::atomic::AtomicUsize,
    /// Решение 403 (ревью t188): сколько записей в `rides` (идущие сделки после учёта покупки, с группой и без) — 0:
    /// очередь расчёта (coin_ride.rs blocks_new) замок `rides` не берёт. Меняется только под замком `rides`.
    pub rides_live: std::sync::atomic::AtomicUsize,
    /// Решение 405 (t191, ревью 3 М2): ID сделок, которые ещё что-то держат — не закрыты (кроме «Беру на себя») или ждут
    /// / ведут возврат USDT (core/unsettled.rs). По нему резерв возвратов (usdt_hold.rs) и монеты других сделок на
    /// счёте (remain_others.rs) — без прохода по всей истории учёта. Блокировка: берётся отдельно, не вместе с `deals`.
    pub unsettled: Mutex<HashSet<String>>,
    /// Решение 414: пары «площадка × тип направления», у которых сейчас идёт снятие слепка баланса для лимита просадки
    /// (core/drawdown_snap.rs) — вторая задача на ту же пару не запускается. Блокировка: берётся отдельно.
    pub draw_busy: Mutex<HashSet<String>>,
    /// Решение 414 (повторное ревью t199): плановые затраты возврата закрываемых сейчас сделок (ID → Cur) — от итога
    /// дня до сохранения сделки в ledger::finish (core/drawdown_pending.rs). Блокировка: берётся отдельно.
    pub draw_closing: Mutex<std::collections::BTreeMap<String, super::drawdown_pending::Cur>>,
    /// Решение 405 (ответ Тиграна на ревью Н3): ручные действия, ждущие окончания идущих сделок монеты, — пауза новых
    /// сделок этой монеты (manual_wait.rs). Блокировка: берётся отдельно (не вместе с `deals` / `rides`).
    pub manual_waits: Mutex<HashMap<super::coin_key::CoinKey, super::manual_wait::ManualWait>>,
    /// Сколько записей в `manual_waits` — 0: входы и очередь расчёта замок не берут (горячий путь).
    pub manual_waits_live: std::sync::atomic::AtomicUsize,
    /// Монета могла освободиться (снята запись идущей сделки, замок монеты, сохранена сделка, отмена ожидания) — будит
    /// задачи ожидания ручных действий (manual_wait_run.rs).
    pub coin_wake: Notify,
    /// Решение 174: монеты, которые сейчас продаются кнопкой «Продать» панели «Балансы» (повторное нажатие — отказ
    /// «уже продаётся»; метка Я-2 в active ставится вместе с этой, balance_sell.rs).
    pub balance_sells: Mutex<HashSet<super::coin_key::CoinKey>>,
    /// Решение 176: память панели «Балансы» (биржи и кошельки DEX) — её читает Telegram (balances_cache.rs).
    pub bal_cache: super::balances_cache::Cache,
    /// Решение 176: идущие ручные переводы USDT по отправителю (повторное нажатие — «уже идёт»; ход — для экрана).
    pub usdt_moves: Mutex<HashMap<String, super::usdt_transfer::Move>>,
    /// Статистика газа по монете (gas_stats.rs).
    pub gas: Mutex<HashMap<Asset, CoinGasStats>>,
    /// Алерты «убыток выше порога» и их сделки (manual.rs).
    pub alerts: Mutex<HashMap<String, (StuckAlert, Deal)>>,
    pub(super) sale_quotes: Mutex<HashMap<String, super::sale_price::Ticket>>,
    /// Алерт «мало USDT» (low_usdt.rs): площадка → ниже порога; площадки, о которых уже записано «не задано».
    pub usdt_below: Mutex<HashMap<VenueId, bool>>,
    pub usdt_alert_unset: Mutex<HashSet<VenueId>>,
    /// Монеты человека («Беру на себя») и сделки, из которых они взяты (manual.rs).
    pub holdings: Mutex<HashMap<String, (ManualHolding, Deal)>>,
    /// Открытые эпизоды арбитражной ситуации (решение 74, episode.rs).
    pub episodes: Mutex<super::episode::Episodes>,
    /// Файлы эпизодов (storage/episode_files.rs): <data-dir>/episodes.
    pub episode_files: crate::storage::episode_files::EpisodeFiles,
    /// Почему эпизоды не ведутся (не заданы episode.*); None — ведутся или ещё не проверялось.
    pub episode_off: Mutex<Option<String>>,
    /// Сухой режим без эпизодов (решение 74): по каким направлениям прошедший уже сообщён в журнал и Telegram —
    /// повтор не пишется, пока направление не перестанет проходить.
    pub dry_told: Mutex<HashSet<Direction>>,
    /// Канал задачи эпизодов (episode_worker.rs): тик и очередь кандидатов только отправляют.
    pub episode_tx: mpsc::Sender<super::episode_run::EpMsg>,
    /// Сколько сообщений задаче эпизодов потеряно (канал полон — тик не ждёт).
    pub episode_lost: std::sync::atomic::AtomicU64,
    /// Приёмник канала — забирает задача эпизодов при запуске ядра (engine.rs).
    pub episode_rx: Mutex<Option<mpsc::Receiver<super::episode_run::EpMsg>>>,
    /// Открытые окна (решение 125, window.rs).
    pub queue_trace_coins: arc_swap::ArcSwap<std::collections::HashSet<super::coin_key::CoinKey>>,
    pub windows: Mutex<super::window::Windows>,
    /// Файлы окон (storage/episode_files.rs, свой экземпляр): <data-dir>/windows.
    pub window_files: crate::storage::episode_files::EpisodeFiles,
    /// Почему окна не пишутся (window.enabled = нет, не задана window.*); None — пишутся или ещё не проверялось.
    pub window_off: Mutex<Option<String>>,
    /// Канал задачи окон (window_worker.rs) и сколько сообщений потеряно (канал полон — тик не ждёт).
    pub window_tx: mpsc::Sender<super::window_run::WinMsg>,
    pub window_lost: std::sync::atomic::AtomicU64,
    /// Приёмник канала — забирает задача окон при запуске ядра (engine.rs).
    pub window_rx: Mutex<Option<mpsc::Receiver<super::window_run::WinMsg>>>,
    /// Толчок задаче окон: направление перестало проходить, сделка сохранена — пора ли закрывать.
    pub window_kick: Notify,
    /// Решение 245: сделки, по которым окно пишет цену DEX на объём сделки (window_deal.rs).
    pub window_deals: super::window_deal::Watch,
    /// Карточки сигнала в работе (card_job.rs): направление → ожидающая следующей работа.
    pub card_jobs: Mutex<HashMap<Direction, Option<super::card_job::Job>>>,
    /// Исполнение циклов DEX ↔ DEX (решение 124): толчок поиска и память причин (cycle_state.rs).
    pub cycles: super::cycle_state::CycleState,
    /// Фоновая очередь расчёта «без потолка» (решение 141, uncapped_state.rs).
    pub uncapped: super::uncapped_state::State,
    /// Делистинги бирж (решение 175, delist.rs).
    pub delist: super::delist::DelistState,
    /// Лимит выводов по бирже (решение 180, withdraw_limit.rs).
    pub withdraw_limit: super::withdraw_limit::State,
    /// Решение 238: снимки стакана по REST для ордера (exec_book.rs) — котировка и худшая цена видят один снимок.
    pub exec_books: super::exec_book::Cache,
    /// Решение 267: замер «что было выгодно, пока монета занята сделкой» — очередь теневых расчётов, отметки
    /// направлений и сводки (busy_calc.rs); при window.busy_calc = нет / не задана — пусто.
    pub busy_calc: super::busy_calc::State,
    /// Решение 334: кандидаты, ждущие места расчёта (profit.calc_wait_ms), и счёт потерь кандидатов (cand_wait.rs).
    pub cand_wait: super::cand_wait::Waiting,
}

impl Inner {
    /// Пустое состояние. Принимает: зависимости, площадки, каналы событий и кандидатов.
    pub fn new(
        deps: Deps,
        kinds: Vec<(VenueId, VenueKind)>,
        market_tx: mpsc::Sender<MarketEvent>,
        cand_tx: mpsc::Sender<(Direction, std::time::Instant)>,
    ) -> Self {
        let (coin_ticks, _) = broadcast::channel(deps.buffer);
        // Канал ограничен (--live-buffer): тик и очередь кандидатов не ждут и не копят память без предела.
        let (episode_tx, episode_rx) = mpsc::channel(deps.buffer);
        let episode_files =
            crate::storage::episode_files::EpisodeFiles::start(deps.data_dir.join("episodes"));
        let (window_tx, window_rx) = mpsc::channel(deps.buffer);
        let window_files = crate::storage::episode_files::EpisodeFiles::start_named(
            deps.data_dir.join("windows"),
            "windows",
        );
        Inner {
            settlement: Default::default(),
            live: Live::new(deps.buffer),
            busy_calc: super::busy_calc::State::new(deps.buffer),
            cand_wait: Default::default(),
            deps,
            market: RwLock::default(),
            networks: RwLock::default(),
            status_pending: RwLock::default(),
            withdraw_fee_refresh: Mutex::default(),
            kinds: RwLock::new(kinds),
            coins: RwLock::default(),
            tickers: RwLock::default(),
            usdt: RwLock::new(Err("справочник бирж ещё не загружен".into())),
            usdt_other: RwLock::new(HashMap::new()),
            blacklist: RwLock::default(),
            excluded_pools: Mutex::default(),
            told_blocks: Mutex::default(),
            held: Mutex::default(),
            not_checked: Mutex::default(),
            first_pass: Mutex::default(),
            signals: Mutex::default(),
            vol: Mutex::default(),
            spreads: RwLock::default(),
            subs: Mutex::default(),
            net_held: Mutex::default(),
            feed_seen: Default::default(),
            ticking: Mutex::default(),
            ripen: Mutex::default(),
            quote_cache: Mutex::default(),
            signal_dex: Default::default(),
            mid_wake: Default::default(),
            spread_sent: Mutex::default(),
            cand_tx,
            queued: Mutex::default(),
            candidates: Mutex::default(),
            last_reject: Default::default(),
            slip_off: Mutex::default(),
            transits: Mutex::default(),
            transit_pairs: Default::default(),
            sales: Default::default(),
            credits: Default::default(),
            exposure: Mutex::default(),
            market_tx,
            kick_networks: Notify::new(),
            networks_gen: std::sync::atomic::AtomicU64::new(0),
            kick_rebuild: Notify::new(),
            coin_ticks,
            risk: Mutex::default(),
            deals: Mutex::default(),
            active: Mutex::default(),
            rides: Mutex::default(),
            groups: Mutex::default(),
            trail: Mutex::default(),
            group_live: Default::default(),
            rides_live: Default::default(),
            unsettled: Mutex::default(),
            draw_busy: Mutex::default(),
            draw_closing: Mutex::default(),
            manual_waits: Mutex::default(),
            manual_waits_live: Default::default(),
            coin_wake: Notify::new(),
            balance_sells: Mutex::default(),
            bal_cache: Default::default(),
            usdt_moves: Mutex::default(),
            gas: Mutex::default(),
            alerts: Mutex::default(),
            sale_quotes: Mutex::default(),
            holdings: Mutex::default(),
            usdt_below: Mutex::default(),
            usdt_alert_unset: Mutex::default(),
            episodes: Mutex::default(),
            episode_files,
            episode_off: Mutex::default(),
            dry_told: Mutex::default(),
            episode_tx,
            episode_lost: Default::default(),
            episode_rx: Mutex::new(Some(episode_rx)),
            queue_trace_coins: Default::default(),
            windows: Mutex::default(),
            window_files,
            window_off: Mutex::default(),
            window_tx,
            window_lost: Default::default(),
            window_rx: Mutex::new(Some(window_rx)),
            window_kick: Notify::new(),
            window_deals: Default::default(),
            card_jobs: Mutex::default(),
            cycles: Default::default(),
            uncapped: Default::default(),
            delist: Default::default(),
            withdraw_limit: Default::default(),
            exec_books: Default::default(),
        }
    }
}
