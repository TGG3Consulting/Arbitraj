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

use crate::core::types::{
    Asset, CoinGasStats, CoinNetworkInfo, Direction, ManualHolding, StuckAlert, VenueId,
};
use crate::gateway::contract::VenueKind;
use crate::gateway::events::MarketEvent;

use super::blacklist::Blacklist;
use super::deal::Deal;
use super::deps::Deps;
use super::limits::Exposure;
use super::market_state::MarketState;
use super::registry::CoinEntry;
use super::risk::Risk;
use super::signal::{Signals, SpreadView};
use super::view::Live;
use super::vol::Volatility;

/// Ключ истории времени в пути: монета, A, B.
pub type TransitKey = (Asset, VenueId, VenueId);

/// Общее состояние ядра.
pub struct Inner {
    pub deps: Deps,
    pub live: Live,
    pub market: RwLock<MarketState>,
    /// Справочник сетей каждой биржи (последний успешный coin_networks).
    pub networks: RwLock<HashMap<VenueId, Vec<CoinNetworkInfo>>>,
    /// Площадки реестра шлюза и их вид (заполняется при старте).
    pub kinds: RwLock<Vec<(VenueId, VenueKind)>>,
    /// Справочник монет и направлений.
    pub coins: RwLock<Vec<CoinEntry>>,
    /// Адрес USDT в BSC или причина, почему его нет.
    pub usdt: RwLock<Result<Address, String>>,
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
    /// Монеты, по которым идёт пересчёт тика: значение true — пришёл ещё тик, пересчитать снова.
    pub ticking: Mutex<HashMap<Asset, bool>>,
    /// Очередь тяжёлого расчёта (candidates.rs) и направления, уже стоящие в ней.
    pub cand_tx: mpsc::Sender<Direction>,
    pub queued: Mutex<HashSet<Direction>>,
    /// Последний расчёт кандидата по направлению: {direction, verdict, at} — для «Живого экрана».
    pub candidates: Mutex<HashMap<Direction, serde_json::Value>>,
    /// Последняя причина отказа по направлению (чтобы не повторять её в журнале).
    pub last_reject: Mutex<HashMap<Direction, String>>,
    /// История времени в пути: (монета, A, B) → длительности переводов по порядку, секунды (transit.rs).
    pub transits: Mutex<HashMap<TransitKey, Vec<Decimal>>>,
    /// Деньги в открытых сделках (limits.rs). Ведёт executor.rs.
    pub exposure: Mutex<Exposure>,
    /// Канал событий адаптеров (передаётся в subscribe_market).
    pub market_tx: mpsc::Sender<MarketEvent>,
    /// «Толчок» обновления сетей бирж (аномальный спред — перечитать статусы ввода и вывода).
    pub kick_networks: Notify,
    /// «Толчок» пересборки справочника (несколько толчков подряд сливаются в одну пересборку).
    pub kick_rebuild: Notify,
    /// Тики монет — для режима «Ожидание» и ожидания котировки (stuck.rs).
    pub coin_ticks: broadcast::Sender<Asset>,
    /// Стоп-условия и режим системы (risk.rs).
    pub risk: Mutex<Risk>,
    /// Сделки по id (ledger.rs).
    pub deals: Mutex<HashMap<String, Deal>>,
    /// Направления, по которым сейчас идёт сделка (одна на направление).
    pub active: Mutex<HashSet<Direction>>,
    /// Статистика газа по монете (gas_stats.rs).
    pub gas: Mutex<HashMap<Asset, CoinGasStats>>,
    /// Алерты «убыток выше порога» и их сделки (manual.rs).
    pub alerts: Mutex<HashMap<String, (StuckAlert, Deal)>>,
    /// Алерт «мало USDT» (low_usdt.rs): площадка → ниже порога; площадки, о которых уже записано «не задано».
    pub usdt_below: Mutex<HashMap<VenueId, bool>>,
    pub usdt_alert_unset: Mutex<HashSet<VenueId>>,
    /// Монеты человека («Беру на себя») и сделки, из которых они взяты (manual.rs).
    pub holdings: Mutex<HashMap<String, (ManualHolding, Deal)>>,
}

impl Inner {
    /// Пустое состояние. Принимает: зависимости, площадки, каналы событий и кандидатов.
    pub fn new(
        deps: Deps,
        kinds: Vec<(VenueId, VenueKind)>,
        market_tx: mpsc::Sender<MarketEvent>,
        cand_tx: mpsc::Sender<Direction>,
    ) -> Self {
        let (coin_ticks, _) = broadcast::channel(deps.buffer);
        Inner {
            live: Live::new(deps.buffer),
            deps,
            market: RwLock::default(),
            networks: RwLock::default(),
            kinds: RwLock::new(kinds),
            coins: RwLock::default(),
            usdt: RwLock::new(Err("справочник бирж ещё не загружен".into())),
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
            ticking: Mutex::default(),
            cand_tx,
            queued: Mutex::default(),
            candidates: Mutex::default(),
            last_reject: Mutex::default(),
            transits: Mutex::default(),
            exposure: Mutex::default(),
            market_tx,
            kick_networks: Notify::new(),
            kick_rebuild: Notify::new(),
            coin_ticks,
            risk: Mutex::default(),
            deals: Mutex::default(),
            active: Mutex::default(),
            gas: Mutex::default(),
            alerts: Mutex::default(),
            holdings: Mutex::default(),
            usdt_below: Mutex::default(),
            usdt_alert_unset: Mutex::default(),
        }
    }
}
