//! Файл: venues/bsc_dex/quote/cpu.rs
//! Что делает: счёт маршрута котировки DEX вне рабочих потоков tokio (решение 129, часть 2 — голодание расчётов при
//! включённой MEXC). Счёт (граф маршрутов, перебор путей, сплит, прямые пулы и пары через промежуточную монету) —
//! чистый CPU без отдачи управления: в рабочем потоке он держал рантайм (таймеры, чтение WebSocket, другие расчёты), а
//! срок расчёта profit.calc_timeout_ms срабатывал, только когда счёт сам отдаст управление. Здесь работа идёт в потоке
//! пула блокирующих задач (spawn_blocking) и ведётся там же (`Handle::block_on`) — редкие запросы к узлу внутри счёта
//! (decimals нового токена, запись в журнал) ждут в этом потоке, а не в рабочем. Вызывающий ждёт итог как обычный
//! await: срок расчёта (tokio::time::timeout ядра) срабатывает вовремя. Сам поток счёта при этом НЕ останавливается
//! (block_on ведёт работу до конца). Решение 301 (в): брошенному счёту поднимается признак (quote/abandon.rs — страж
//! здесь, у ожидания вызывающего); перед тяжёлыми шагами — ожидание замка построения графа (pools/graph_cache.rs;
//! начатое построение достраивается и кладётся в кэш, проверка — после него: уточнение t137), отбор лучших путей и
//! сплит (quote/graph_route.rs)
//! — счёт проверяет признак и выходит сразу. Шаг, который уже идёт (глубина одного пула, ожидание замка, начатое до
//! срока, классика котировки, запрос к узлу), доживает до своего конца; итог брошенного счёта выбрасывается.
//! Класс срочности (urgency), счётчик запросов расчёта (calc_meter) и признак финальной
//! котировки (quote_stage) — признаки задачи tokio — переносятся в поток счёта; решение 348 — и назначение котировки
//! (gateway/quote_purpose.rs scope_opt).
//! Предел одновременных счётов — dex.quote_cpu_max (необязательная; 0 или не задана — без предела): сверх него счёт
//! ждёт места (ожидание — внутри срока расчёта и видно в замере этапов как время котировки без запросов к узлу).
//! Решение 338 (задача t157): задана cpu.threads_max — вместо места dex.quote_cpu_max берётся место общего предела
//! потоков счёта (gateway/cpu_budget.rs) ярусом задачи: финальная котировка, котировка исполнения или задача сделки —
//! путь сделки; кандидат (класс Trade или ярус вызывающего) — кандидат; прочее — фон. dex.quote_cpu_max при этом не
//! берётся: очередь по порядку прихода перед приоритетным местом давала инверсию — кандидаты, ждущие общего места,
//! держали места dex.quote_cpu_max, и котировка сделки стояла за ними (ревью t157 Н1). Не задана cpu.threads_max —
//! dex.quote_cpu_max, как до 336. Ярус и короб подбора объёма ядра (gateway/calc_share.rs) переносятся в поле потока
//! (quote/cpu_local.rs) только когда есть что переносить (предел задан или короб есть) — иначе как до 336.
//! Кто вызывает: quote/mod.rs (Quoting::quote — выбор маршрута), venue.rs (dex_cycles; dex_cycle_quote при
//! параллельных точках подбора).
//! От чего зависит: ctx.rs, quote/abandon.rs, quote/cpu_local.rs, gateway/{urgency, calc_meter, quote_stage, patience,
//! cpu_budget, calc_share, quote_purpose}.rs, tokio.
//! Настройки: dex.quote_cpu_max [bsc_dex] (только без cpu.threads_max); cpu.threads_max, cpu.reserve_deal, cpu.reserve_calc
//! (вся система).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::gateway::calc_meter::Meter;
use crate::gateway::cpu_budget::Tier;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::urgency::Urgency;
use crate::gateway::{
    calc_meter, calc_share, cpu_budget, patience, quote_purpose, quote_stage, urgency,
};

use super::super::ctx::Ctx;
use super::{abandon, cpu_local};

/// Предел одновременных счётов: (значение настройки, семафор). Один на процесс (адаптер DEX один).
static LIMIT: Mutex<Option<(u64, Arc<Semaphore>)>> = Mutex::new(None);

/// Место под счёт по dex.quote_cpu_max. None — предела нет (не задана или 0). Смена значения на лету — новый
/// семафор: идущие счёты доживают на старом (кратко возможно старый предел + новый). Ошибок нет.
async fn permit(ctx: &Ctx) -> Option<OwnedSemaphorePermit> {
    let n = ctx.int("dex.quote_cpu_max").ok().filter(|n| *n > 0)?;
    let sem = {
        let mut g = LIMIT.lock().unwrap_or_else(|p| p.into_inner());
        match g.as_ref() {
            Some((k, s)) if *k == n => s.clone(),
            _ => {
                let s = Arc::new(Semaphore::new(usize::try_from(n).unwrap_or(usize::MAX)));
                *g = Some((n, s.clone()));
                s
            }
        }
    };
    sem.acquire_owned().await.ok()
}

/// Выполняет работу котировки вне рабочих потоков tokio. Принимает: окружение (настройка предела, текст ошибки),
/// работу — из кучи (`Pin<Box<F>>`, владеет своими данными). Возвращает: её итог.
/// Ошибки: её ошибки; Internal — поток счёта упал (паника) или рантайм остановлен.
/// Решение 301 (дополнение — стек): работа принимается только из кучи. Будущее работы (routing::run: классика, граф,
/// повтор графа решения 300) иначе лежало бы по значению: в состоянии этого future (через permit().await) и выше — в
/// состоянии вызывающего (Quoting::quote, venue quote) на стеке рабочего потока tokio; и в потоке блокирующих задач
/// (2 МиБ по умолчанию) — в замыкании и в цепочке обёрток-признаков (scoped), которая собирается на стеке до block_on
/// (сам block_on кладёт в кучу только готовое будущее больше порога tokio). Каждая обёртка — async fn, хранит и
/// аргумент, и будущее, его содержащее (rust-lang/rust#62958). Замер (tests/bsc_dex_quote_cpu_depth.rs, run
/// 43776945366): по значению цепочка занимала 2047728 из 2097152 байт стека потока счёта до начала работы, из кучи —
/// 23488. Сторож — tests/bsc_dex_quote_cpu_stack.rs.
pub async fn run<F, T>(ctx: &Ctx, work: Pin<Box<F>>) -> GatewayResult<T>
where
    F: Future<Output = GatewayResult<T>> + Send + 'static,
    T: Send + 'static,
{
    // Решение 301 (в): страж живёт в этом future — бросили ожидание (срок расчёта), признак поднят, счёт выходит.
    let (_abandon, flag) = abandon::guard();
    let class = urgency::is_set();
    let meter = calc_meter::current();
    let fin = quote_stage::is_final();
    // Решение 348 (слияние t157 + t160): назначение «кто вызвал котировку» — в поток счёта, как класс срочности.
    let purpose = quote_purpose::current();
    // Решение 338: задан общий предел — место по ярусу (без dex.quote_cpu_max, ревью Н1); не задан — как до 336.
    let (held, budget, tier) = match (
        cpu_budget::is_optional_quote(),
        cpu_budget::limits(&ctx.settings),
    ) {
        (true, limits) => {
            let p = cpu_budget::try_optional(limits).ok_or_else(|| {
                ctx.err(
                    ErrorCode::Internal,
                    "нет свободного фонового места для необязательной котировки".to_owned(),
                )
            })?;
            (None, Some(p), Some(Tier::Background))
        }
        (false, Some(lim)) => {
            let tier = cpu_budget::tier_of(class, fin || patience::is_exec());
            // Ожидание места — из кучи: будущее run не растёт (сторож стека tests/bsc_dex_quote_cpu_stack.rs).
            (
                None,
                Some(Box::pin(cpu_budget::acquire(lim, tier)).await),
                Some(tier),
            )
        }
        (false, None) => (permit(ctx).await, None, None),
    };
    let share = calc_share::current();
    let rt = tokio::runtime::Handle::current();
    let job = tokio::task::spawn_blocking(move || {
        let _held = held;
        let _budget = budget;
        // Поле потока — только когда есть что переносить (ревью Б1).
        let _local = (tier.is_some() || share.is_some())
            .then(|| cpu_local::enter(tier.unwrap_or(Tier::Background), share));
        rt.block_on(quote_purpose::scope_opt(
            purpose,
            scoped(flag, class, meter, fin, work),
        ))
    });
    job.await.map_err(|e| {
        ctx.err(
            ErrorCode::Internal,
            format!("счёт котировки DEX прерван: {e}"),
        )
    })?
}

/// Будущее, которое run ведёт в потоке счёта (block_on): работа под четырьмя признаками задачи — брошенный расчёт
/// (abandon), класс срочности (urgency), счётчик запросов (calc_meter), финальная котировка (quote_stage).
/// Работа принимается только из кучи (`Pin<Box<F>>`): размер итога не зависит от размера работы — указатель и
/// обёртки (сторож — tests/bsc_dex_quote_cpu_stack.rs). pub — точка входа для тестов-сторожей (боевой вызов — run).
/// Принимает: признак, класс, счётчик, признак финальной, работу. Возвращает: будущее с итогом работы. Ошибок нет
/// (ошибки — у работы).
pub fn scoped<F: Future>(
    flag: Arc<AtomicBool>,
    class: Option<Urgency>,
    meter: Option<Arc<Meter>>,
    fin: bool,
    work: Pin<Box<F>>,
) -> impl Future<Output = F::Output> {
    abandon::scope(
        flag,
        urgency::scope_opt(
            class,
            calc_meter::scope_opt(meter, quote_stage::final_if(fin, work)),
        ),
    )
}
