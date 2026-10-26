//! Файл: core/dry.rs
//! Что делает: «сухой режим» (решение 74) — сигналы без денег. Включён (dry.enabled = да): кандидат считается от
//! виртуального бюджета dry.budget_usdt вместо свободного баланса площадки A (лимиты money.* действуют как обычно,
//! limits.rs), прошедший кандидат помечается «сухой» и уходит в журнал, Telegram и эпизоды, а исполнитель НЕ
//! вызывается: ни ордеров, ни переводов, ни перегона USDT. Сделка, начатая до включения, доводится как обычно.
//! Переключение — на лету: настройка читается на каждом кандидате и на каждом входе исполнителя.
//! dry.enabled не задана — обычный режим, как до решения 74 (на плашке — «сухой режим не задан»). Значение не
//! читается (повреждение) — считаем режим сухим: при сомнении денег не трогаем.
//! Режим фиксируется один раз на расчёт кандидата и передаётся явно (limits.rs, profit.rs): у живой сделки (executor.rs,
//! precheck.rs) режим всегда Live — виртуального бюджета у неё не бывает; перед покупкой — ещё раз blocks().
//! Кто вызывает: core/candidates.rs (режим расчёта), core/executor.rs (вход исполнителя и перед покупкой),
//! core/usdt_return.rs (перегон «отдельно»), api (status — плашка интерфейса), тесты.
//! От чего зависит: settings.
//! Настройки: dry.enabled, dry.budget_usdt.

use rust_decimal::Decimal;
use serde::Serialize;

use crate::gateway::errors::ErrorCode;
use crate::settings::read::At;
use crate::settings::store::Store;

/// Режим ядра для новых действий.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Mode {
    /// Обычный: деньги настоящие, исполнитель работает.
    Live,
    /// Сухой: виртуальный бюджет (Err — почему его нет: сигналов нет, причина видна).
    Dry(Result<Decimal, String>),
}

impl Mode {
    /// Сухой ли режим. Ошибок нет.
    pub fn is_dry(&self) -> bool {
        matches!(self, Mode::Dry(_))
    }
}

/// Что делает: читает режим из настроек сейчас (на лету).
/// Принимает: хранилище настроек. Возвращает: Mode. Ошибок нет — всё в Mode.
pub fn mode(s: &Store) -> Mode {
    match s.flag("dry.enabled", At::default()) {
        Ok(false) => Mode::Live,
        Err(e) if e.code == ErrorCode::NotConfigured => Mode::Live,
        Ok(true) => Mode::Dry(
            s.decimal("dry.budget_usdt", At::default())
                .map_err(|e| format!("сухой режим: {}", e.message)),
        ),
        Err(e) => Mode::Dry(Err(format!(
            "сухой режим: dry.enabled не читается ({}) — ничего не исполняется",
            e.message
        ))),
    }
}

/// Что делает: замок для любого нового действия с деньгами (сделка, перегон USDT).
/// Принимает: хранилище настроек. Возвращает: None — можно; Some(причина) — сухой режим, не делать. Ошибок нет.
pub fn blocks(s: &Store) -> Option<String> {
    mode(s)
        .is_dry()
        .then(|| "сухой режим (dry.enabled): ордера и переводы не выполняются".to_string())
}

/// Режим для плашки интерфейса (GET /api/dry): ядро решает, интерфейс только показывает.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// «on» — сухой с бюджетом; «nobudget» — сухой без dry.budget_usdt; «broken» — сухой: dry.enabled не читается;
    /// «off» — обычный (dry.enabled = нет); «unset» — dry.enabled не задана, обычный.
    pub state: &'static str,
    pub budget: Option<Decimal>,
    /// Причина для «nobudget» и «broken».
    pub reason: Option<String>,
}

/// Что делает: режим для плашки. Принимает: хранилище настроек. Возвращает: Status. Ошибок нет.
pub fn status(s: &Store) -> Status {
    let st = |state, budget, reason| Status {
        state,
        budget,
        reason,
    };
    match s.flag("dry.enabled", At::default()) {
        Ok(false) => st("off", None, None),
        Err(e) if e.code == ErrorCode::NotConfigured => st("unset", None, None),
        Err(e) => st(
            "broken",
            None,
            Some(format!("значение dry.enabled не читается: {}", e.message)),
        ),
        Ok(true) => match s.decimal("dry.budget_usdt", At::default()) {
            Ok(b) => st("on", Some(b), None),
            Err(e) => st("nobudget", None, Some(e.message)),
        },
    }
}
