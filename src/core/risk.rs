//! Файл: core/risk.rs
//! Что делает: стоп-условия и управление (шаг 6 ТЗ, экран «Управление»): режим системы (старт, стоп,
//! пауза), пауза площадки после N ошибок подряд, карантин монеты, остановка по убытку за день,
//! пауза площадки при расхождении сверки (решение 221: Risk.reconcile_venues). Торговля не включается, пока не заданы
//! все настройки групп money.* и risk.* (решение 15); risk.max_errors_in_row — и у площадки продажи B (решение 118, Я-10).
//! Карантин — по адресу контракта (quarantine.rs, решение 118).
//! Лимит убытка за день — по площадке (daily_venue_limit.rs, решение 173: пауза только площадки) и общий «Стоп»
//! (daily_limit.rs, risk.max_daily_loss_total_usd; решения 53, 152, 153). Пауза циклов DEX ↔ DEX по убытку дня DEX —
//! Risk.cycle_paused: отказ только направлениям без биржи (DEX ↔ DEX). t91: учёт дня сделки биржа ↔ биржа — у биржи
//! покупки, но ВХОД не менялся: пауза любой из двух площадок направления (Risk.venue_paused) не даёт начать сделку;
//! площадки входа — daily_venue::entry_venues. Необязательные настройки (optional) не
//! требуются для включения торговли. Решение 225: кто поставил паузу (софт — Risk.venue_auto / Risk.mode_auto,
//! auto_pause.rs); софтовую снимает сам бот (auto_resume.rs, day_roll.rs), человек — сбрасывает отметку.
//! Решение 230: пауза кошелька DEX от сверки (Risk.wallet_paused, wallet_pause.rs) — отказ направлениям этого кошелька.
//! Решение 249: направление биржа → биржа, запрещённое venue.cex_cex / venue.cex_cex_deny (cex_link.rs), — отказ.
//! Кто вызывает: executor.rs (перед сделкой и при ошибках), daily_limit.rs (стоп по лимиту дня), transfers.rs
//! (карантин), api (команды управления через методы Core).
//! От чего зависит: engine.rs, settings (catalog, read), view.rs, wallet_queue_gate.rs (очередь кошелька, решение 231 п. 4).
//! Настройки: risk.max_errors_in_row (по площадке), risk.max_daily_loss_usd (по площадке; у каждой биржи
//! направления, DEX ↔ DEX — у DEX: daily_venue::entry_venues), risk.max_daily_loss_total_usd (необязательна),
//! все обязательные money.* и risk.*.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rust_decimal::Decimal;

use serde::{Deserialize, Serialize};

use crate::core::types::{Direction, VenueId};
use crate::settings::read::At;

pub use super::day_view::DayView;
use super::engine::{Core, lk};
use super::view::CoreEvent;

/// Режим системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Новые сделки не открываются; после запуска система в этом режиме до команды «Старт».
    Stopped,
    Running,
    /// Новые сделки не открываются, идущие доводятся до конца.
    Paused,
}

/// Состояние риска — для экрана «Управление».
#[derive(Debug, Clone, Serialize)]
pub struct Risk {
    pub mode: Mode,
    /// Почему система не в работе (кто или что остановил).
    pub reason: Option<String>,
    pub venue_paused: HashMap<VenueId, String>,
    pub errors_in_row: HashMap<VenueId, u64>,
    /// Монеты в карантине — все их направления на паузе до ручного снятия. Ключ — адрес контракта (решение 118,
    /// quarantine.rs); в интерфейс — списком.
    #[serde(serialize_with = "super::quarantine::serialize")]
    pub quarantine: super::quarantine::Map,
    /// Итог дня (решения 53, 152, daily.rs): дата, всего, из них факт и оценка позиций; лимит дня (daily_limit.rs).
    pub day: DayView,
    /// Система остановлена именно по общему лимиту дня (daily_limit.rs); любая смена режима снимает отметку.
    pub daily_stop: bool,
    /// Решение 221: площадки, паузу которых (venue_paused) поставила сверка балансов — reconcile_pause.rs снимет её
    /// сама, когда по площадке расхождений нет. Человек или другой модуль поставил или снял паузу площадки — отметка
    /// снимается (forget_reconcile), такую паузу снимает только человек.
    #[serde(skip)]
    pub reconcile_venues: HashSet<VenueId>,
    /// Решение 173: площадки на паузе по убытку дня (их причина — в venue_paused). Снимает «Снять паузу» или
    /// «Сбросить лимит дня» (по площадке или общий).
    #[serde(skip)]
    pub loss_paused: BTreeSet<String>,
    /// Решение 173: циклы DEX ↔ DEX на паузе по убытку дня DEX (id DEX → причина); направления CEX ↔ DEX этой
    /// паузой не останавливаются. Снимает «Снять паузу» DEX или «Сбросить лимит дня».
    pub cycle_paused: BTreeMap<String, String>,
    /// Решение 173: убыток площадки для лимита при прошлом пересчёте — пауза ставится при переходе через лимит или
    /// при росте убытка (daily_venue_limit.rs). Новый день — с нуля.
    #[serde(skip)]
    pub loss_seen: BTreeMap<String, Decimal>,
    /// Решение 225: паузы площадок, поставленные софтом (вид, когда) — их снимает сам бот (auto_resume.rs). Нет записи —
    /// пауза человека или другого модуля. «Снять паузу» человеком снимает отметку.
    pub venue_auto: HashMap<VenueId, super::auto_pause::VenueMark>,
    /// Решение 225: «Стоп» системы поставлен софтом (общий лимит дня, правило «стоп всё») — режим до него; любая смена
    /// режима (risk::set_mode) снимает отметку.
    pub mode_auto: Option<super::auto_pause::ModeMark>,
    /// Решение 231: открытые «исход неизвестен» по площадкам (outcome_open.rs) — паузу Outcome бот снимает, когда все
    /// выяснены им самим. «Снять паузу» человеком снимает учёт.
    #[serde(skip)]
    pub outcome_open: HashMap<VenueId, super::outcome_open::Open>,
    /// Решение 230: кошельки DEX на паузе от сверки балансов (id кошелька → кто и почему, wallet_pause.rs): новые
    /// сделки этим кошельком (маппинг, решение 132) не начинаются, остальные кошельки торгуют. Ставит и сама снимает
    /// сверка (reconcile_wallet.rs), вручную — «Снять паузу» кошелька.
    pub wallet_paused: BTreeMap<String, super::wallet_pause::WalletPause>,
}

impl Default for Risk {
    fn default() -> Self {
        Risk {
            mode: Mode::Stopped,
            reason: Some("система запущена — ждёт команды «Старт»".into()),
            venue_paused: HashMap::new(),
            errors_in_row: HashMap::new(),
            quarantine: HashMap::new(),
            day: DayView::default(),
            daily_stop: false,
            reconcile_venues: HashSet::new(),
            loss_paused: BTreeSet::new(),
            cycle_paused: BTreeMap::new(),
            loss_seen: BTreeMap::new(),
            venue_auto: HashMap::new(),
            mode_auto: None,
            outcome_open: HashMap::new(),
            wallet_paused: BTreeMap::new(),
        }
    }
}

/// Публикует состояние управления в интерфейс. Ошибок нет.
pub fn publish(core: &Core) {
    let r = snapshot(core);
    if let Ok(v) = serde_json::to_value(r) {
        core.inner.live.send(CoreEvent::Control(v));
    }
}

/// Можно ли открыть новую сделку по направлению. Ошибки: текст причины.
pub fn can_open(core: &Core, d: &Direction) -> Result<(), String> {
    super::risk_ready::settings_ready(core, d)?;
    // Решение 249: биржа → биржа по venue.cex_cex обеих и списку venue.cex_cex_deny биржи покупки (сразу по настройкам,
    // не дожидаясь пересборки справочника).
    super::cex_link::direction_ok(core, d)?;
    // Решение 230: кошелёк направления по текущему маппингу — до блокировки риска (свои замки видов и маппинга).
    let wallet = super::dex_account::of_direction_now(core, d);
    // Решение 231 (дополнение, п. 4): очередь кошелька остановлена — «биржа → DEX» не начинается (до замка риска).
    let queue_halt = super::wallet_queue_gate::halted_for(core, d, wallet.as_ref());
    let r = lk(&core.inner.risk);
    if r.mode != Mode::Running {
        // Решение 164: режим по-русски («Стоп», «Пауза»), а не внутреннее имя.
        let why = r.reason.clone().unwrap_or_default();
        let why = if why.is_empty() {
            String::new()
        } else {
            format!(" — {why}")
        };
        return Err(format!(
            "торговля в режиме «{}»{why}",
            super::commands_control::label(r.mode)
        ));
    }
    for v in [&d.buy_on, &d.sell_on] {
        if let Some(why) = r.venue_paused.get(v) {
            return Err(format!("площадка {} на паузе: {why}", v.0));
        }
    }
    // Решение 230: пауза кошелька DEX от сверки — только направлениям, которые пойдут этим кошельком.
    if let Some(p) = wallet.and_then(|w| r.wallet_paused.get(&w.0)) {
        return Err(p.refusal());
    }
    if let Some(why) = queue_halt {
        return Err(why);
    }
    // Решение 173: пауза циклов DEX ↔ DEX по убытку дня DEX — только у направлений без биржи (DEX ↔ DEX). t91 вход не
    // менял: площадки входа — daily_venue::entry_venues (прежнее правило), не площадка учёта дня.
    for v in super::daily_venue::entry_venues(core, d) {
        if let Some(why) = r.cycle_paused.get(&v) {
            return Err(format!("циклы DEX ↔ DEX на паузе: {why}"));
        }
    }
    if let Some(q) = r.quarantine.get(&super::coin_key::CoinKey::of(&d.asset)) {
        return Err(format!("{} в карантине: {}", q.label, q.reason));
    }
    Ok(())
}

/// Команда управления (интерфейс): режим системы. Возвращает: текст для журнала.
pub fn set_mode(core: &Core, mode: Mode, reason: Option<String>) -> String {
    {
        let mut r = lk(&core.inner.risk);
        r.mode = mode;
        r.reason = reason.clone();
        r.daily_stop = false;
        // Решение 225: режим сменили — софтовая отметка «Стоп» снята (правило «стоп всё» ставит её после этой смены).
        r.mode_auto = None;
    }
    publish(core);
    format!(
        "Режим системы: {mode:?}{}",
        reason.map(|r| format!(" — {r}")).unwrap_or_default()
    )
}

/// Ошибка площадки: считает подряд; достигнут risk.max_errors_in_row — пауза площадки.
/// Возвращает: Some(текст) — площадка поставлена на паузу (для уведомления risk.stop).
pub fn on_error(core: &Core, venue: &VenueId) -> Option<String> {
    let max = core.inner.deps.settings.integer(
        "risk.max_errors_in_row",
        At {
            venue: Some(&venue.0),
            coin: None,
        },
    );
    let text = {
        let mut r = lk(&core.inner.risk);
        let n = r.errors_in_row.entry(venue.clone()).or_default();
        *n += 1;
        let n = *n;
        match max {
            Ok(m) if n >= m => {
                let t = format!("{n} ошибок подряд — предел {m}");
                let now = chrono::Utc::now();
                super::auto_pause::pause_venue(
                    &mut r,
                    venue,
                    t.clone(),
                    super::auto_pause::Kind::Errors,
                    now,
                );
                Some(t)
            }
            _ => None,
        }
    };
    publish(core);
    text
}

/// Успешная операция площадки — счётчик ошибок подряд обнуляется.
pub fn on_ok(core: &Core, venue: &VenueId) {
    lk(&core.inner.risk).errors_in_row.remove(venue);
}

/// Снять паузу площадки (интерфейс). Ошибок нет.
pub fn resume_venue(core: &Core, venue: &VenueId) {
    let mut r = lk(&core.inner.risk);
    r.venue_paused.remove(venue);
    // Решение 221: паузу снял человек — отметка сверки снята.
    forget_reconcile(&mut r, venue);
    r.errors_in_row.remove(venue);
    // Решение 173: пауза по убытку дня тоже снята; новая — только если убыток площадки вырастет.
    r.loss_paused.remove(&venue.0);
    r.cycle_paused.remove(&venue.0);
    // Решение 225: паузу снял человек — софтовая отметка снята.
    r.venue_auto.remove(venue);
    // Решение 231: учёт «исход неизвестен» площадки — тоже (человек проверил сам).
    r.outcome_open.remove(venue);
    drop(r);
    publish(core);
}

/// Решение 221: паузу площадки поставил или снял не сверка (человек, другой модуль) — отметка «паузу поставила
/// сверка» снимается, автоснятия нет. Вызывать под блокировкой риска. Принимает: состояние риска, площадку.
/// Возвращает: true — отметка стояла (пауза площадки была от сверки). Ошибок нет.
pub fn forget_reconcile(r: &mut Risk, venue: &VenueId) -> bool {
    r.reconcile_venues.remove(venue)
}

// Проверка дневного лимита убытка и «Сбросить лимит дня» — core/daily_limit.rs (решения 53, 152, 153).

/// Состояние для экрана «Управление»; лимиты дня — по текущим настройкам (daily_limit::view, daily_venue_limit::view).
pub fn snapshot(core: &Core) -> Risk {
    let mut r = lk(&core.inner.risk).clone();
    r.day.limit = super::daily_limit::view(core, &r.day);
    let date = r.day.date;
    for (v, x) in r.day.venues.iter_mut() {
        x.limit = super::daily_venue_limit::view(core, v, x, date);
        x.paused = r.loss_paused.contains(v);
    }
    r
}
