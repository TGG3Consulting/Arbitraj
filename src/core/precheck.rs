//! Файл: core/precheck.rs
//! Что делает: предпроверка сразу перед покупкой (шаг 5 ТЗ): стоп-условия (risk.rs); статусы вывода на A
//! и депозита на B — перечитываются напрямую из API бирж; покупка через API на A и продажа на B — по свежему справочнику
//! пар (pairs_fresh.rs); у биржи B — адрес депозита монеты (решение 118, precheck_deposit.rs); BNB на газ у DEX-сторон не ниже
//! risk.min_bnb_balance; затем полный расчёт заново (profit.rs) по свежим стаканам и пулам; биржа B примет ордер
//! продажи на объём плана (решение 134, precheck_sell.rs), продажа DEX B — симуляцией настоящей продажи кошельком сделки
//! с пересчётом по её выходу (решение 234, precheck_dex_sell.rs); на бирже A хватит средств, которые можно вывести (решение 150,
//! precheck_withdraw.rs).
//! Решение 265 (включаемое, log.step_timing = да): кроме прежней строки «время: …» — «Время по шагам (предпроверка): …»
//! и этапы пересчёта «Время по шагам (пересчёт с финальной котировкой, всего N мс): …» (step_timing.rs) — в журнал
//! core.precheck и хронологию сделки (executor.rs); выключено — только прежняя строка, замера этапов нет.
//! Решение 266 (Тигран 13.11, четыре части, каждая под своей настройкой; не заданы или «нет» — шаги как раньше):
//! precheck.fast_recalc = да — пересчёт считает на свежих данных только объём кандидата (precheck_fast.rs), не прошёл —
//! полный подбор; строка «План: …» (каким путём получен план) — в хронологию сделки, журнал core.precheck и к тексту
//! отказа; precheck.parallel = да — запросы сторон A и B и удержания вывода A идут одновременно (precheck_par.rs),
//! итоги разбираются в прежнем порядке шагов, в строках времени — пометка «шли одновременно»;
//! precheck.networks_memory = да — справочник сетей биржи берётся из памяти ядра, пока её записи монеты моложе
//! интервала фонового опроса (precheck_nets.rs), иначе — запрос, как раньше; источник — в «Время по шагам»;
//! precheck.balance_first = да — свободный баланс A читается здесь, прямо перед расчётом (precheck_balance.rs), расчёт
//! идёт с пределом min(бюджет, свободно); исполнитель баланс повторно не читает.
//! Решение 411 (t196): при precheck.parallel = да шаг «адрес депозита B» идёт одновременно с пересчётом и проверкой
//! продажи (от них не зависит); адрес из памяти адаптера (под ключом API) — без запроса, запрос — по триггеру
//! (precheck_deposit.rs, deposit_proof.rs).
//! Решение 392 (t180): ценовые коридоры бирж A и B (price_band.rs context: только у пар с правилом коридора, обе
//! стороны одновременно; ревью Н3 — одновременно и с шагами сторон, своя строка «коридор цены» во времени шагов); пересчёт держит худшие цены плана внутри коридора; строки коридора — в журнал
//! core.precheck и хронологию сделки всегда.
//! Кто вызывает: executor.rs.
//! От чего зависит: networks_merge.rs (решение 387), step_timing.rs, risk.rs, profit.rs, convert.rs, trade_ops.rs (вид площадки), deps.rs (шлюз),
//! precheck_fast.rs, precheck_par.rs, precheck_nets.rs, precheck_balance.rs.
//! Настройки: risk.min_bnb_balance, venue.symbols_stale_s (+ всё, что читают risk.rs и profit.rs); включатели решения
//! 266 (precheck.fast_recalc, precheck.parallel, precheck.networks_memory, precheck.balance_first) читает executor.rs
//! (precheck_fast.rs read).

use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, Direction, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::costs::{Calc, CostLine};
use super::engine::Core;
use super::precheck_balance::{Bal, Cand, Read};
use super::precheck_fast::{Opts, Want};
use super::precheck_nets::Src;
use super::profit::{self, Verdict};

/// Статус ввода или вывода монеты по сети BSC прямо из API биржи. Ошибка — текст причины.
/// Решение 266, часть 3: `nets` — Some при precheck.networks_memory = да: записи монеты в памяти ядра моложе интервала
/// фонового опроса площадки (precheck_nets.rs recent) — запроса к бирже нет, проверка — по ним тем же кодом и текстами;
/// иначе запрос, как раньше; в `nets` пишется источник. None — всегда запрос (как до решения 266).
pub(super) async fn transfer_open(
    core: &Core,
    v: &VenueId,
    d: &Direction,
    withdraw: bool,
    nets: Option<&mut Src>,
) -> Result<(), String> {
    // Решение 211: справочник площадки — по её тикеру монеты; сеть ищется по адресу.
    let on = super::venue_ticker::ticker_on(core, &d.asset, v);
    let mem = match nets {
        Some(src) => {
            let m = super::precheck_nets::recent_in(
                core,
                v,
                &on,
                d.asset.chain.as_ref(),
                d.asset.contract,
            );
            *src = m.as_ref().map_or(Src::Http, |(_, age)| Src::Memory(*age));
            m.map(|(list, _)| list)
        }
        None => None,
    };
    let stored = mem.is_some();
    let list = match mem {
        Some(list) => list,
        None => {
            let gw = core.inner.deps.gw.get(v).await.map_err(|e| e.message)?;
            // Решение 113: справочник одной монеты (BingX — config/getall?coin=; у кого запроса одной монеты нет —
            // весь справочник с фильтром, как раньше).
            let list = gw
                .coin_networks_of(&on)
                .await
                .map_err(|e| format!("{}: справочник сетей: {}", v.0, e.message))?;
            // Нативная монета ↔ обёртка (решение 62): тот же справочник, что у ядра (native.rs).
            // Решение 431: записи — к сети ядра (net_chain.rs), затем нативная монета.
            super::net_chain::prepare(core, v, list)
        }
    };
    let open = list
        .iter()
        // Решение 431: запись — по сети и адресу монеты (адрес в другой сети — другая монета, core/net_chain.rs).
        .find(|i| {
            i.asset.contract.is_some()
                && i.asset.contract == d.asset.contract
                && i.asset.chain == d.asset.chain
        })
        .map(|i| {
            if withdraw {
                i.withdraw_enabled
            } else {
                i.deposit_enabled
            }
        });
    // Свежие записи этой монеты — в справочник ядра вместо её старых; остальные монеты биржи не трогаются (решение 117:
    // после решения 113 здесь приходит справочник ОДНОЙ монеты, а он целиком заменял справочник биржи — остальные
    // монеты теряли сеть BSC до следующего опроса). Справочника биржи ещё нет — не пишем (его соберёт registry_sync).
    // Решение 266, часть 3: записи взяты из памяти — писать нечего, справочник не менялся.
    // Решение 387: запись — networks_merge.rs (тот же код, вынесен для запроса минимума депозита deposit_min_ask.rs).
    if !stored {
        super::networks_merge::merge_coin(core, v, &on, list);
    }
    match open {
        Some(true) => Ok(()),
        Some(false) => Err(format!(
            "{}: {} {} закрыт",
            v.0,
            d.asset.ticker,
            if withdraw { "вывод" } else { "ввод" }
        )),
        None => Err(format!(
            "{}: у {} нет сети BSC с тем же адресом контракта",
            v.0, d.asset.ticker
        )),
    }
}

/// Сторона сделки по справочнику пар биржи (pairs_fresh.rs): на A (buy = true) покупка через API разрешена, на B —
/// продажа; справочник не старше venue.symbols_stale_s. Направление могло быть поставлено до смены флагов —
/// здесь оно отсекается до ордера. Ошибка — текст причины.
pub(super) fn pair_side(core: &Core, v: &VenueId, d: &Direction, buy: bool) -> Result<(), String> {
    let limit = super::pairs_fresh::limit(&core.inner.deps.settings, v);
    let t = super::venue_ticker::ticker_on(core, &d.asset, v);
    let market = super::engine::rd(&core.inner.market);
    super::pairs_fresh::side(&market, v, &t, buy, &limit, chrono::Utc::now())
}

/// BNB на газ у DEX-площадки: свободный нативный BNB ≥ резерв risk.min_bnb_balance + газ транзакций сделки на ней
/// (need; сверка балансов — 0). Решение 132: кошелёк — строки маппинга направления. Принимает: ядро, направление,
/// площадку, газ сделки. Ошибка — текст (мало BNB, резерв не задан, нет кошелька в маппинге, баланс не получен).
pub async fn bnb_ok(core: &Core, d: &Direction, v: &VenueId, need: Decimal) -> Result<(), String> {
    let min = core
        .inner
        .deps
        .settings
        .decimal("risk.min_bnb_balance", At::default())
        .map_err(|e| e.message)?;
    // Решение 416: тикер газа — dex.gas_ticker этой площадки DEX (core/gas_ticker.rs); не задана — точная причина.
    let gas = super::gas_ticker::at(&core.inner.deps.settings, v)?;
    let gw = super::dex_account::venue_dir(&core.inner.deps, d, v).await?;
    // Решение 113: только BNB (у DEX — eth_getBalance), а не балансы всех токенов кошелька (~11 с на вызов).
    let have = gw
        .balance(&gas)
        .await
        .map_err(|e| format!("{}: балансы: {}", v.0, e.message))?
        .map_or(Decimal::ZERO, |x| x.free);
    bnb_enough(&v.0, have, min, need)
}

/// Хватает ли BNB на газ: свободный BNB не меньше минимума risk.min_bnb_balance плюс газ сделки. Принимает: площадку,
/// свободный BNB, минимум, газ сделки. Ошибка — текст причины. Вызывают bnb_ok и сверка (reconcile.rs — по уже
/// полученным балансам, без второго запроса).
pub fn bnb_enough(who: &str, have: Decimal, min: Decimal, need: Decimal) -> Result<(), String> {
    if have < min + need {
        // Решение 164: числа для глаз (количество BNB — 4 значащие цифры).
        use crate::notify::human::qty;
        return Err(format!(
            "{who}: BNB на газ {} — меньше нужного {} (запас {} + газ сделки {})",
            qty(have),
            qty(min + need),
            qty(min),
            qty(need)
        ));
    }
    Ok(())
}

/// Газ (BNB) транзакций сделки на одной площадке: строки расчёта её стадий с газом в BNB (газ обмена, разрешений,
/// перевода, обёртки, чаевые; у «возврат USDT» — газ перевода USDT, convert::usdt_return). Решение 416: `gas` — тикер
/// газа площадки (dex.gas_ticker). Ошибок нет.
pub fn gas_need(costs: &[CostLine], stages: &[&str], gas: &str) -> Decimal {
    costs
        .iter()
        .filter(|l| stages.contains(&l.stage) && l.asset == gas)
        .filter(|l| super::convert::is_gas(l.kind))
        .map(|l| l.amount)
        .sum()
}

/// Предпроверка. Возвращает: свежий расчёт для сделки и строки «Время по шагам» для хронологии (решение 265: только при
/// on — log.step_timing = да, иначе пусто). Ошибки: текст причины отказа (с временем шагов, как до решения 265).
/// Решение 111: время каждого шага (справочник сетей биржи, баланс BNB, пересчёт с финальной котировкой) — в текст
/// отказа и в журнал при успехе: 02.11 у RVV между кандидатом и отказом прошло 16 с, а на что — по журналу не видно.
/// Решение 265: при on к записи журнала добавляются и строки «Время по шагам» (стоп-условия; этапы пересчёта).
/// Решение 266: при precheck.fast_recalc = да первой строкой — «План: …» (каким путём получен план; от
/// log.step_timing не зависит), она же — в журнал и к тексту отказа, если пересчёт дошёл до быстрого пути.
/// Часть 4 (precheck.balance_first = да): третьим в итоге — прочитанный свободный баланс A для исполнителя (Bal; иначе
/// None); биржа A баланс не отдала — строка об этом в журнал (в хронологию её ставит executor.rs).
/// Принимает: ядро, направление, включён ли замер по шагам, режим решения 266 (оба читает executor.rs один раз на
/// сделку; Opts::default() — как до решения 266), счёт DEX сделки (для баланса кошелька DEX A в части 4).
pub async fn run(
    core: &Core,
    d: &Direction,
    on: bool,
    fast: Opts,
    acc: Option<&DexAccountId>,
) -> Result<(Calc, Vec<String>, Option<Bal>), String> {
    // К этому моменту направление и торговый счёт уже выбраны. Удерживаем интерес
    // только до завершения этой предпроверки, в том числе при отмене её future.
    let _presell_interest = if let Some(account) = acc && d.asset.contract.is_some() {
        core.inner.deps.gw.get_as(&d.sell_on, Some(account)).await.ok()
            .and_then(|venue| venue.warm_presell(&d.asset))
    } else { None };
    let mut t = Timing {
        on,
        ..Timing::default()
    };
    let res = super::precheck_hint::during(
        core,
        d,
        fast.qty,
        Box::pin(steps(core, d, &mut t, &fast, acc)),
    )
    .await;
    // Решение 380: строка урезания — в хронологию всегда (не зависит от log.step_timing).
    let mut lines: Vec<String> = t
        .plan
        .clone()
        .into_iter()
        .chain(t.cut.clone())
        .chain(t.band.clone())
        .chain(t.lines())
        .collect();
    if res.is_ok() {
        let extra: String = (t.missed.iter().chain(lines.iter()))
            .map(|l| format!(". {l}"))
            .collect();
        core.inner
            .deps
            .log(
                crate::storage::journal::Level::Info,
                "core.precheck",
                Some(&d.buy_on.0),
                format!(
                    "{} {} → {}: предпроверка пройдена ({}){extra}",
                    d.asset.ticker,
                    d.buy_on.0,
                    d.sell_on.0,
                    t.text()
                ),
                None,
            )
            .await
    }
    // Решение 265 (при on): у отказа — и строка этапов пересчёта, если до него дошли.
    // Решение 266: и строка «План: …» (precheck.fast_recalc = да и пересчёт дошёл до быстрого пути).
    // Решение 392: и строки коридора цены (если читались).
    let calc: String = (t
        .missed
        .iter()
        .chain(t.band.iter())
        .chain(t.plan.iter())
        .chain(t.calc.iter()))
    .map(|l| format!(". {l}"))
    .collect();
    // Подробности уже записаны адаптерной предпроверкой. Ссылка остаётся в карточке даже при последующем refit.
    lines.extend(t.presell_receipt.take());
    res.map(|(c, bal)| (c, lines, bal))
        .map_err(|e| format!("{e} ({}){calc}", t.text()))
}

/// Время шагов предпроверки: (что, мс) — как до решения 265, всегда. Решение 265 (только при on): время стоп-условий и
/// строка этапов пересчёта.
#[derive(Default)]
struct Timing {
    presell_receipt: Option<String>,
    steps: Vec<(String, u128)>,
    on: bool,
    stop_ms: Option<u128>,
    calc: Option<String>,
    /// Решение 266: строка «План: …» — каким путём получен план (только при precheck.fast_recalc = да).
    plan: Option<String>,
    /// Решение 266: шаги сторон и удержаний шли одновременно (precheck.parallel = да) — пометка в строках времени.
    par: Option<&'static str>,
    /// Решение 266, часть 3: источник справочника сетей шага (имя шага, «запрос» / «из памяти, возраст N мс») — в
    /// подробный замер («Время по шагам (предпроверка)»).
    src: Vec<(String, String)>,
    /// Решение 266, часть 4: биржа A не отдала свободный баланс — строка для журнала core.precheck.
    missed: Option<String>,
    /// Решение 380 (ревью t166, Н6): объём урезан до проверенного симуляцией продажи — строка хронологии сделки.
    cut: Option<String>,
    /// Решение 392 (t180): коридоры цены бирж, прочитанные перед пересчётом (или почему не прочитаны) — в хронологию
    /// сделки и журнал всегда (не зависит от log.step_timing).
    band: Vec<String>,
    /// Решение 411: шаг «адрес депозита B» шёл одновременно с пересчётом и проверкой продажи — пометка в строках
    /// времени (только если шаг был).
    par_addr: bool,
}

/// Решение 266: пометка строк времени при precheck.parallel = да.
const PAR_NOTE: &str = "шаги сетей, BNB и удержаний вывода шли одновременно (precheck.parallel) — сумма шагов не равна \
                        общему времени";
/// То же при precheck.balance_first = да: одновременно читался и свободный баланс A.
const PAR_NOTE_BAL: &str = "шаги сетей, BNB, удержаний вывода и свободного баланса шли одновременно \
                            (precheck.parallel) — сумма шагов не равна общему времени";
/// Решение 411: добавка к пометке, если шаг адреса депозита B шёл одновременно с пересчётом.
const PAR_ADDR: &str =
    "адрес депозита — одновременно с пересчётом и проверкой продажи (решение 411)";

impl Timing {
    /// Запоминает шаг. Принимает: название, момент начала. Ошибок нет.
    fn mark(&mut self, what: String, from: std::time::Instant) {
        self.steps.push((what, from.elapsed().as_millis()));
    }

    /// «время: справочник сетей bingx 820 мс, пересчёт 14100 мс» (как до решения 265). Ошибок нет.
    fn text(&self) -> String {
        let parts: Vec<String> = self
            .steps
            .iter()
            .map(|(w, ms)| format!("{w} {ms} мс"))
            .collect();
        let par = self.note().map(|n| format!("; {n}")).unwrap_or_default();
        format!("время: {}{par}", parts.join(", "))
    }

    /// Пометка «шли одновременно» (решение 266) с добавкой про адрес депозита (решение 411, только если шаг был
    /// одновременно с пересчётом). Ошибок нет.
    fn note(&self) -> Option<String> {
        let n = self.par?;
        Some(if self.par_addr {
            format!("{n}; {PAR_ADDR}")
        } else {
            n.to_string()
        })
    }

    /// Решение 265: строки «Время по шагам (предпроверка): стоп-условия N мс; …» и этапов пересчёта; выключено — пусто.
    /// Ошибок нет.
    fn lines(&self) -> Vec<String> {
        if !self.on {
            return Vec::new();
        }
        let mut s = super::step_timing::Steps::new(true);
        if let Some(ms) = self.stop_ms {
            s.add(|| ("стоп-условия".into(), ms as i128));
        }
        for (w, ms) in &self.steps {
            // Решение 266, часть 3: у шага «справочник сетей X» — источник (только при precheck.networks_memory = да).
            let name = match self.src.iter().find(|(n, _)| n == w) {
                Some((_, src)) => format!("{w} ({src})"),
                None => w.clone(),
            };
            s.add(|| (name, *ms as i128));
        }
        s.text("предпроверка")
            .map(|l| match self.note() {
                Some(n) => format!("{l}; {n}"),
                None => l,
            })
            .into_iter()
            .chain(self.calc.clone())
            .collect()
    }
}

/// Шаги предпроверки с замером времени. Принимает: ядро, направление, замер, режим решения 266 (всё выключено — шаги
/// как раньше), счёт DEX сделки. Возвращает: расчёт и (часть 4) прочитанный баланс A. Ошибки: текст причины отказа.
async fn steps(
    core: &Core,
    d: &Direction,
    t: &mut Timing,
    fast: &Opts,
    acc: Option<&DexAccountId>,
) -> Result<(Calc, Option<Bal>), String> {
    use std::time::Instant;
    // Решение 265: время стоп-условий — только при включённом замере по шагам.
    let at = t.on.then(Instant::now);
    let r = super::risk::can_open(core, d);
    t.stop_ms = at.map(|a| a.elapsed().as_millis());
    r?;
    // Решение 266 (precheck.parallel = да): шаги сторон и запрос удержаний вывода A — одновременно (precheck_par.rs),
    // итоги — в прежнем порядке шагов; удержания (held) решаются ниже, на прежнем месте. Иначе — по очереди, как раньше.
    let mut held = None;
    // Часть 4: свободный баланс A, прочитанный перед расчётом (None — режим выключен).
    let mut read: Option<Read> = None;
    // Решение 411 (precheck.parallel): место шага «адрес депозита B» во времени шагов — сразу после шагов сторон (как
    // в последовательном коде: перед «свободный баланс» и «коридор цены»); отказ чтения баланса, когда адрес B ещё
    // проверяется, — после адреса (порядок последовательного кода: адрес B раньше баланса).
    let mut addr_at = 0;
    let mut bal_err: Option<String> = None;
    let free_step = || format!("свободный баланс {}", d.buy_on.0);
    // Решение 392 (ревью t180, Н3): ценовые коридоры бирж A и B читаются одновременно с шагами сторон (сети, адрес
    // депозита, BNB, удержания, баланс) — не отдельным шагом после них; своя строка в «Время по шагам».
    let band = async {
        let at = Instant::now();
        let r = Box::pin(super::price_band::read(core, d)).await;
        Ok::<_, String>((r, at.elapsed().as_millis()))
    };
    let sides = async {
        if fast.parallel {
            let par = super::precheck_par::Par {
                nets: fast.networks_memory,
                balance: fast.balance.is_some(),
                acc,
            };
            let (marks, src, r) = super::precheck_par::sides(core, d, par).await;
            // Пометка «шли одновременно» — только если запросы были (вид площадки A не найден — шагов нет).
            let note = if fast.balance.is_some() {
                PAR_NOTE_BAL
            } else {
                PAR_NOTE
            };
            t.par = (!marks.is_empty()).then_some(note);
            t.steps.extend(marks);
            t.src.extend(src);
            let (h, bal, due) = r?;
            held = h;
            addr_at = t.steps.len();
            // Часть 4: итог чтения баланса — на своём месте, перед расчётом (после шагов сторон).
            if let Some((r, ms)) = bal {
                t.steps.push((free_step(), ms));
                match r {
                    Ok(x) => read = Some(x),
                    Err(e) if due => bal_err = Some(e),
                    Err(e) => return Err(e),
                }
            }
            // Решение 411: адрес депозита B проверить — ниже, одновременно с пересчётом и проверкой продажи.
            Ok::<bool, String>(due)
        } else {
            for (v, withdraw) in [(&d.buy_on, true), (&d.sell_on, false)] {
                match super::trade_ops::kind(core, v)? {
                    VenueKind::Cex => {
                        let at = Instant::now();
                        let mut src = Src::Http;
                        let nets = fast.networks_memory.then_some(&mut src);
                        let r = transfer_open(core, v, d, withdraw, nets).await;
                        let step = format!("справочник сетей {}", v.0);
                        if fast.networks_memory {
                            t.src.push((step.clone(), src.text()));
                        }
                        t.mark(step, at);
                        r?;
                        pair_side(core, v, d, withdraw)?;
                        // Решение 118 (Я-5): у биржи-получателя адрес депозита — до покупки, а не после.
                        if !withdraw {
                            let at = Instant::now();
                            let r = super::precheck_deposit::ready(core, d).await;
                            t.mark(format!("адрес депозита {}", v.0), at);
                            r?;
                        }
                    }
                    VenueKind::Dex => {
                        let at = Instant::now();
                        let r = bnb_ok(core, d, v, Decimal::ZERO).await;
                        t.mark(format!("баланс BNB {}", v.0), at);
                        if let Err(e) = r {
                            core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
                            return Err(e);
                        }
                    }
                }
            }
            // Часть 4 (precheck.balance_first = да): свободный баланс A — последним шагом перед расчётом.
            if fast.balance.is_some() {
                let at = Instant::now();
                let r = super::precheck_balance::read(core, d, acc).await;
                t.mark(free_step(), at);
                read = Some(r?);
            }
            Ok(false)
        }
    };
    // Ревью 2 t180, Н-C2: отказ стороны возвращается сразу — чтение коридора при этом отменяется (try_join).
    let unit_checks = async {
        Ok::<_, String>(tokio::join!(
            super::settlement_unit::confirmed(core, d),
            super::precheck_deposit::withdraw_destination(core, d, acc)
        ))
    };
    let ((mut bands, band_ms), addr_due, (units_result, destination_result)) =
        tokio::try_join!(band, sides, unit_checks)?;
    // Решение 411: баланс A отказал, адрес B ещё не проверен — сначала адрес (его ошибка первая по порядку).
    if let Some(e) = bal_err {
        let at = Instant::now();
        let r = super::precheck_deposit::ready(core, d).await;
        let step = format!("адрес депозита {}", d.sell_on.0);
        t.steps.insert(addr_at, (step, at.elapsed().as_millis()));
        r?;
        return Err(e);
    }
    // Строка времени — только если коридор читался (есть строки коридора).
    if !bands.lines.is_empty() {
        t.steps.push((
            "коридор цены (одновременно с шагами сторон)".into(),
            band_ms,
        ));
    }
    // Ревью 2 t180, Б1: контекст расчёта с коридорами — только теперь, после шагов сторон: справочник сетей уже
    // обновлён ими (transfer_open → networks_merge), комиссии и минимумы вывода / депозита — свежие.
    let pre = super::price_band::context(core, d, &mut bands);
    t.band = bands.lines;
    if let Some(Read::Missed(why)) = &read {
        t.missed = Some(super::precheck_balance::missed_note(d, why));
    }
    // Часть 4: предел расчёта — свободный баланс (в расчёте — min(бюджет, свободно), формула buy_cap); свободного
    // меньше, чем нужно кандидату (критерий refit::needed), — сразу полный подбор под остаток (быстрый путь его не
    // считает: Want::spend). Баланс не получен (биржа) — как будто хватает: предел — бюджет.
    let free = read.as_ref().and_then(Read::free);
    let cand = fast.balance.filter(|_| free.is_some());
    let short = cand.filter(|c| free.is_some_and(|f| c.short(f)));
    // Решение 411 (t196, precheck.parallel = да): адрес депозита биржи B не зависит от пересчёта и проверки продажи —
    // идёт одновременно с ними (живой факт 18.11 ESIM bsc_dex → kucoin: «адрес депозита kucoin 284 мс» + «проверка
    // продажи kucoin 298 мс» шли по очереди). Итог — в прежнем порядке: ошибка адреса — первая (как будто пересчёта не
    // было: его шаги и строки снимаются); время шага адреса — сразу после шагов сторон (addr_at), как в последовательном коде.
    let addr = async {
        if !addr_due {
            return None;
        }
        let at = Instant::now();
        let r = super::precheck_deposit::ready(core, d).await;
        let step = format!("адрес депозита {}", d.sell_on.0);
        Some((step, at.elapsed().as_millis(), r))
    };
    let calc = Box::pin(async {
        destination_result?;
        let verified_units = units_result.as_ref().map_err(Clone::clone)?;
        let at = Instant::now();
        // Будущее расчёта большое — в куче: обёртки замера иначе копируют его на стек (переполнение в отладочной сборке).
        // Решение 265: этапы пересчёта по шагам — только при включённом замере (выключен — работа без обёрток).
        // Решение 266 (precheck.fast_recalc = да): сначала — только объём кандидата; не прошёл — полный подбор (profit.rs).
        let mut want = fast
            .qty
            .filter(|_| verified_units.is_identity())
            .map(|q| Want {
                spend: cand.map(|c| c.spend_max),
                ..Want::new(q)
            });
        // Решение 392 (t180): план считается внутри коридора, прочитанного выше одновременно с шагами сторон (контекст —
        // на справочнике сетей после шагов сторон).
        let plan = Box::pin(replan_pre(core, d, free, want.as_mut(), pre));
        let (verdict, m) = super::step_timing::measure_if(t.on, plan).await;
        t.mark("пересчёт с финальной котировкой".into(), at);
        t.calc = m.map(|m| m.calc_text("пересчёт с финальной котировкой"));
        t.plan = want.and_then(|w| w.line());
        let c = match (verdict, &read, short) {
            (Verdict::Accept(c), _, _) => c,
            // Часть 4: свободного меньше, чем нужно кандидату, и под остаток невыгодно — причина «свободный баланс».
            // Только отказ по порогам прибыли (money.min_profit_usd / pct) на объёме под остаток: его profit.rs отдаёт с
            // расчётом, у которого записан выбор объёма (best с size — так помечается только он); прочие отказы
            // пересчёта (делистинг, закрытые входы, налоги, нет цены, бюджет, отказ подбора и финальной котировки) —
            // прежним текстом: по типу Verdict::Reject их от «не про баланс» не отличить.
            (Verdict::Reject { reason, best }, Some(Read::Got { free, quote }), Some(c))
                if super::precheck_balance::by_thresholds(best.as_ref()) =>
            {
                return Err(super::precheck_balance::short_reject(
                    d, *free, quote, &c, &reason,
                ));
            }
            (Verdict::Reject { reason, .. }, _, _) => {
                return Err(format!("пересчёт перед покупкой: {reason}"));
            }
        };
        // Решение 134: биржа B примет ордер продажи на объём плана — до покупки (precheck_sell.rs). Решение 234: у DEX B —
        // симуляция настоящей продажи кошельком сделки, расчёт — по её выходу (precheck_dex_sell.rs).
        let at = Instant::now();
        // Решение 380 (ревью t166, Н6): план до проверки продажи — чтобы урезание по симуляции было видно своей строкой.
        let (pre, pre_sell) = (Cand::of(&c), c.sell_qty);
        let r = super::precheck_sell::check_traced(core, d, c, &mut t.presell_receipt).await;
        t.mark(format!("проверка продажи {}", d.sell_on.0), at);
        let c = r?;
        let cut = (c.sell_qty < pre_sell).then_some(pre);
        t.cut = cut.map(|p| super::precheck_dex_cut::chrono(&p, pre_sell, &c));
        Ok::<_, String>((c, cut))
    });
    let (addr, calc) = tokio::join!(addr, calc);
    if let Some((step, ms, r)) = addr {
        t.steps.insert(addr_at, (step, ms));
        t.par_addr = true;
        if let Err(e) = r {
            t.steps.truncate(addr_at + 1);
            (t.calc, t.plan, t.cut) = (None, None, None);
            return Err(e);
        }
    }
    let (c, cut) = calc?;
    let verified_units = units_result?;
    if c.units_b_per_a.buy_per_raw != verified_units.buy_per_raw
        || c.units_b_per_a.sell_per_raw != verified_units.sell_per_raw
    {
        return Err("во время пересчёта изменились единицы площадок, ожидается свежий план".into());
    }
    // Часть 4: инвариант пары buy_cap + refit — списание плана по худшей цене не больше предела списания
    // (min(бюджет плана, свободно)); расчёт его держит сам (final_calc), здесь — сверка после проверки продажи.
    if let Some(free) = free {
        let limit = c.size.as_ref().map_or(free, |s| s.budget.min(free));
        if c.spend_max > limit {
            return Err(format!(
                "пересчёт перед покупкой: списание по худшей цене {} больше предела списания {limit} (свободно {free})",
                c.spend_max.round_dp(4)
            ));
        }
    }
    // Решение 150: на бирже A хватит выводимого (без предварительно зачисленных депозитов) — до покупки.
    // Решение 266: удержания получены заранее (precheck.parallel) — здесь только решение; время шага — запрос + решение.
    // Часть 4: свободный баланс для этой проверки — уже прочитанный (один запрос баланса на сделку).
    let a_cex = super::trade_ops::kind(core, &d.buy_on) == Ok(VenueKind::Cex);
    match held {
        Some((got, ms)) => {
            let at = Instant::now();
            let r = match got {
                Ok(g) => super::precheck_withdraw::settle(core, &d.buy_on, g, &c, free).await,
                Err(e) => Err(e),
            };
            let ms = ms + at.elapsed().as_millis();
            t.steps
                .push((format!("удержания вывода {}", d.buy_on.0), ms));
            r?;
        }
        None if free.is_some() && a_cex => {
            let at = Instant::now();
            let a = &d.buy_on;
            let r = match super::precheck_withdraw::fetch(core, a, false).await {
                Ok(g) => super::precheck_withdraw::settle(core, a, g, &c, free).await,
                Err(e) => Err(e),
            };
            t.mark(format!("удержания вывода {}", d.buy_on.0), at);
            r?;
        }
        None => {
            let at = Instant::now();
            let r = super::precheck_withdraw::check(core, d, &c).await;
            t.mark(format!("удержания вывода {}", d.buy_on.0), at);
            r?;
        }
    }
    // Решение 62: газ всех транзакций сделки на DEX — сверх резерва, иначе после покупки BNB перевод на биржу
    // упадёт с InsufficientBalance и монета застрянет. A — покупка и перевод, B — продажа и возврат USDT.
    for (v, stages) in [
        (&d.buy_on, ["A", "перевод"]),
        (&d.sell_on, ["B", super::costs_types::RETURN_STAGE]),
    ] {
        if super::trade_ops::kind(core, v)? != VenueKind::Dex {
            continue;
        }
        let at = Instant::now();
        let gas = super::gas_ticker::at(&core.inner.deps.settings, v)?;
        let r = bnb_ok(core, d, v, gas_need(&c.costs, &stages, &gas)).await;
        t.mark(format!("газ BNB {}", v.0), at);
        if let Err(e) = r {
            core.inner.deps.notify("risk.low_bnb", Some(&v.0), &e).await;
            return Err(e);
        }
    }
    let bal = read.map(|read| Bal {
        read,
        refit: short,
        cut,
    });
    Ok((c, bal))
}

/// Что делает: пересчёт живой сделки перед покупкой — всегда в режиме Live (решение 74): виртуального бюджета у сделки не
/// бывает, даже если сухой режим включили сейчас. Принимает: ядро, направление. Возвращает: Verdict. Ошибок нет.
pub async fn replan(core: &Core, d: &Direction) -> Verdict {
    // Решение 348: назначение «предпроверка» (замер «кто вызвал котировку»).
    let work = profit::decide(core, d, &super::dry::Mode::Live);
    crate::gateway::quote_purpose::scope(crate::gateway::quote_purpose::Purpose::Precheck, work)
        .await
}

/// Что делает: тот же пересчёт (режим Live) с объёмом кандидата — решение 266 (precheck.fast_recalc = да): сначала один
/// финальный расчёт на этом объёме, не прошёл — полный подбор; None — ровно replan. Принимает: ядро, направление,
/// предел бюджета (часть 4, precheck.balance_first = да: свободный баланс A — как у refit::plan; None — без предела),
/// запрос быстрого пути (в него пишется исход). Возвращает: Verdict. Ошибок нет.
pub async fn replan_sized(
    core: &Core,
    d: &Direction,
    cap: Option<Decimal>,
    want: Option<&mut Want>,
) -> Verdict {
    replan_pre(core, d, cap, want, None).await
}

/// Что делает: replan_sized с готовым контекстом направления (решение 392: с прочитанными коридорами цены бирж —
/// core/price_band.rs context). Принимает: как replan_sized и контекст (None — собрать как раньше).
/// Возвращает: Verdict. Ошибок нет.
pub async fn replan_pre(
    core: &Core,
    d: &Direction,
    cap: Option<Decimal>,
    want: Option<&mut Want>,
    pre: Option<Box<super::dirctx::DirCtx>>,
) -> Verdict {
    // Решение 348: назначение «предпроверка» (замер «кто вызвал котировку»); решение 345 — без мягкого срока.
    let work = profit::decide_capped(core, d, &super::dry::Mode::Live, cap, want, pre, None);
    crate::gateway::quote_purpose::scope(crate::gateway::quote_purpose::Purpose::Precheck, work)
        .await
}
