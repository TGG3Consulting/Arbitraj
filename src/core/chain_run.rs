//! Файл: core/chain_run.rs
//! Что делает: решение 432 (шаг 4 двух сетей, Тигран 19.11.2026) — запуск и остановка торговли по сети: кнопки «Старт
//! <сеть>» / «Стоп <сеть>» рядом с общими «Старт» (все сети) / «Пауза» / «Стоп». Общий режим Risk.mode не меняется по
//! смыслу (все софтовые остановки, лимиты, автоснятие — прежние); сети, остановленные человеком, — Risk.chain_off.
//! Сделка направления сети X начинается, только если режим «В работе» и X нет в chain_off (risk::can_open) — так же, как
//! общий «Стоп»: новые не начинаются, идущие (перевод, продажа, возврат USDT, перегон, ручные) доводятся.
//! Правила: «Старт сети X» при режиме не «В работе» — режим «В работе», запущена только X (остальные — в chain_off);
//! при режиме «В работе» — X убирается из chain_off. «Стоп сети X» — X в chain_off; запущенных сетей не осталось —
//! общий «Стоп». Общий «Старт» человека (Core::set_mode_by) очищает chain_off — запущены все сети.
//! **Одна сеть площадок DEX — сетей для кнопок нет (chains = []), chain_off всегда пуст, can_open настроек сверх
//! прежнего не читает: поведение и плашка — как до решения 432.**
//! Имена площадок DEX для человека («DEX» / «DEX <сеть>») — publish_names (Core::start).
//! Кто вызывает: core/engine.rs (publish_names), core/risk.rs (can_open — gate_input, refusal; snapshot — chains, view), core/commands.rs
//! (Core::set_chain_by — set), тесты tests/core_chain_run_432.rs.
//! От чего зависит: core/chain.rs (сети площадок DEX, сеть направления), core/risk.rs (Risk, Mode, put_mode, publish).
//! Настройки: нет своих (сеть площадки — dex.chain, решение 416).

use serde::Serialize;

use crate::core::types::{ChainId, Direction};

use super::engine::{Core, lk};
use super::risk::{Mode, Risk};

/// Сеть для плашки: id, торгует ли сейчас, почему нет.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainRun {
    pub id: String,
    pub running: bool,
    /// Почему не торгует (сеть остановлена или общий режим не «В работе»); None — торгует.
    pub why: Option<String>,
}

/// Что делает: сети площадок DEX для кнопок по сети — dex.chain каждой площадки DEX, без повторов, по порядку реестра.
/// Принимает: ядро. Возвращает: список; меньше двух сетей — пусто (одна сеть: кнопок по сети нет, настройки при одной
/// площадке DEX не читаются). Ошибок нет: площадка, чья dex.chain не задана, сети не даёт.
pub fn chains(core: &Core) -> Vec<ChainId> {
    let dex = super::chain::dexes(core);
    if dex.len() < 2 {
        return Vec::new();
    }
    let s = &core.inner.deps.settings;
    let mut out: Vec<ChainId> = Vec::new();
    for v in &dex {
        if let Ok(c) = super::chain::chain_at(s, v)
            && !out.contains(&c)
        {
            out.push(c);
        }
    }
    if out.len() < 2 { Vec::new() } else { out }
}

/// Что делает: сеть направления для проверки входа — только если какая-то сеть остановлена (иначе None: ничего не
/// читается, как до решения 432). Принимает: ядро, направление. Возвращает: None — проверка не нужна; Some(Ok(сеть)) или
/// Some(Err(причина)) — сеть не прочитать. Ошибок нет. Вызывать до замка риска (сеть читается из настроек).
pub fn gate_input(core: &Core, d: &Direction) -> Option<Result<ChainId, String>> {
    if lk(&core.inner.risk).chain_off.is_empty() {
        return None;
    }
    Some(super::chain::of_dir(core, d))
}

/// Что делает: отказ входу по сети направления. Принимает: состояние риска, сеть (или причину, почему её не прочитать).
/// Возвращает: Some(текст отказа) — сеть остановлена или сеть не прочитать при остановленной сети (не торгуем
/// вслепую); None — сеть торгует. Ошибок нет.
pub fn refusal(r: &Risk, c: Result<ChainId, String>) -> Option<String> {
    match c {
        Ok(c) => r
            .chain_off
            .get(&c.0)
            .map(|why| format!("сеть {c} остановлена: {why}")),
        Err(e) => Some(format!(
            "сеть направления не прочитать, а торговля по сетям остановлена частично: {e}"
        )),
    }
}

/// Что делает: запуск или остановка сети в состоянии риска (правила — в шапке). Принимает: состояние, сети площадок DEX
/// (chains), сеть, запуск ли, кто (причина для плашки и журнала). Возвращает: текст для журнала.
/// Ошибки: текст — сетей меньше двух (кнопки по сети не нужны: «Старт»/«Стоп»), такой сети нет у площадок DEX.
pub fn apply(r: &mut Risk, all: &[ChainId], c: &ChainId, run: bool, who: &str) -> Result<String, String> {
    if all.len() < 2 {
        return Err("одна сеть площадок DEX — запуск и остановка кнопками «Старт» / «Стоп»".into());
    }
    if !all.contains(c) {
        let list: Vec<&str> = all.iter().map(ChainId::as_str).collect();
        return Err(format!("сети {c} нет среди сетей площадок DEX ({})", list.join(", ")));
    }
    if run {
        if r.mode != Mode::Running {
            super::risk::put_mode(r, Mode::Running, Some(format!("запущена сеть {c} — {who}")));
            r.chain_off = all
                .iter()
                .filter(|x| *x != c)
                .map(|x| (x.0.clone(), format!("не запускалась: запущена только сеть {c} — {who}")))
                .collect();
        } else {
            r.chain_off.remove(&c.0);
        }
        return Ok(format!("Сеть {c}: старт — {who}"));
    }
    r.chain_off.insert(c.0.clone(), format!("остановлена — {who}"));
    if r.mode == Mode::Running && all.iter().all(|x| r.chain_off.contains_key(&x.0)) {
        super::risk::put_mode(r, Mode::Stopped, Some(format!("остановлены все сети — {who}")));
    }
    Ok(format!("Сеть {c}: стоп — {who}"))
}

/// Что делает: сети для плашки. Принимает: состояние риска, сети площадок DEX (chains). Возвращает: строки; одна сеть —
/// пусто. Ошибок нет.
pub fn view(r: &Risk, all: &[ChainId]) -> Vec<ChainRun> {
    all.iter()
        .map(|c| {
            let why = match (r.mode, r.chain_off.get(&c.0)) {
                (_, Some(w)) => Some(w.clone()),
                (Mode::Running, None) => None,
                (m, None) => Some(format!(
                    "режим системы «{}»",
                    super::commands_control::label(m)
                )),
            };
            ChainRun {
                id: c.0.clone(),
                running: why.is_none(),
                why,
            }
        })
        .collect()
}

/// Что делает: команда человека «Старт / Стоп сети» — состояние риска и плашка. Принимает: ядро, id сети, запуск ли,
/// кто. Возвращает: текст для журнала. Ошибки: как у apply.
pub fn set(core: &Core, chain: &str, run: bool, who: &str) -> Result<String, String> {
    let all = chains(core);
    let t = apply(
        &mut lk(&core.inner.risk),
        &all,
        &ChainId(chain.to_string()),
        run,
        who,
    )?;
    super::risk::publish(core);
    Ok(t)
}

/// Что делает: имена площадок DEX для человека (notify/venue_names.rs): одна сеть — «DEX» (как раньше), сетей несколько —
/// «DEX <сеть>» (dex.chain; не задана — «DEX <id площадки>»). Вызывает Core::start. Принимает: ядро. Ошибок нет.
pub fn publish_names(core: &Core) {
    let many = !chains(core).is_empty();
    let s = &core.inner.deps.settings;
    let names = super::chain::dexes(core).into_iter().map(|v| {
        let name = if many {
            match super::chain::chain_at(s, &v) {
                Ok(c) => format!("DEX {c}"),
                Err(_) => format!("DEX {}", v.0),
            }
        } else {
            "DEX".to_string()
        };
        (v.0, name)
    });
    crate::notify::venue_names::set_dex(names);
}
