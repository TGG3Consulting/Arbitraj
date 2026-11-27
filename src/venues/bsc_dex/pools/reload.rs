//! Файл: venues/bsc_dex/pools/reload.rs
//! Что делает: чтение состояния пула пачки на блоке с отправкой ядру (load_one) и повтор чтения пулов, которые не
//! прочитались (сбой узла): на каждом новом блоке (опрос блоков node/heads.rs), не больше dex.pool_load_retries раз. Журнал: одна запись
//! «не прочитан», одна — «прочитан с попытки N» или «так и не прочитан за N попыток».
//! Многие пулы сразу (приём пачки, повтор) — load_many: пачками через Multicall3 (load_batch.rs, решение 61).
//! Кто вызывает: pools/session.rs, pools/admit.rs, per_block*.rs (load_one_block, put_emit). Возврат брошенных из-за
//! настройки — revive.rs.
//! Прочитанный (или окончательно не прочитанный) пул уходит из «загружается» (loading.rs).
//! От чего зависит: pools/load.rs, loading.rs, listen_log.rs (emit, note_refill, found_usdt), registry.rs, progress.rs.
//! Решение 339 (pool_fail.rs): при dex.pool_fail_pause_ms пул на паузе после неудач в этот блок не читается (попытка не
//! тратится), прочая (не StaleData) неудача повтора удлиняет паузу. Ревью t153 (Б1): при dex.pool_fail_pause_ms пул,
//! не прочитанный за dex.pool_load_retries попыток не из-за незаданной настройки, НЕ бросается — одна запись журнала и
//! «недостоверен» вместо «загружается», а повтор идёт дальше с паузой (StaleData после предела — тоже с паузой);
//! частоту ограничивают пауза и её потолок. Так рабочий пул после сбоя узла возвращается сам, без переподключения.
//! Не задана — как раньше: брошен до переподключения / настройки / события, требующего перечитывания.
//! Задача t160 (dex.per_block_skip_same, необязательная): чтение «каждым блоком» (load_one_block, put_emit,
//! reload_many::load_group_block), у которого поколение состояния (state_gen.rs) после записи то же и не 0, ядру не
//! шлётся — состояние не изменилось. Первое чтение (записи не было) и возврат из недостоверного (valid входит в
//! поколение) — всегда новое поколение, событие уходит. Действует только при dex.pool_events_keep = да: без неё событие
//! пула при полном канале ядра теряется (решение 313), и повторная отправка каждым блоком — единственная доставка.
//! Не задана (или dex.pool_events_keep не «да») — как раньше, событие на каждое чтение.
//! Записи «не прочитано за N попыток» — с адресом пула (depth_book::label, как у dex.pool_refresh): под одной подписью
//! «вид тикер» бывает несколько пулов.
//! Настройки: dex.pool_load_retries, dex.per_block_skip_same [bsc_dex]; через pool_fail.rs — dex.pool_fail_pause_ms,
//! dex.pool_fail_pause_max_ms.

use crate::storage::journal::Level;

use crate::gateway::errors::GatewayError;

use super::depth_book::label;
use super::discover::Found;
use super::listen::{End, Pools};
use super::registry::Entry;
use super::{listen_log, load};

/// Настройка задачи t160: не слать ядру неизменившееся состояние пула «каждым блоком».
pub const SKIP_SAME: &str = "dex.per_block_skip_same";

pub use super::revive::Abandoned;

/// Пул, ждущий повторного чтения: индекс в пачке и сколько повторов уже было.
pub struct Missing {
    pub i: usize,
    pub tries: u64,
}

/// Повтор чтения ждущих пулов на блоке `at`. Исчерпавшие попытки — в `abandoned` (брошены до переподключения или до
/// изменения настроек — session.rs revive). Не задана dex.pool_load_retries — ждущие остаются ждать (повтор после её
/// появления). Ошибки: End — ядро закрыло канал.
pub async fn retry(
    p: &Pools,
    pools: &[(Found, String)],
    missing: &mut Vec<Missing>,
    abandoned: &mut Vec<Abandoned>,
    at: u64,
) -> Result<(), End> {
    let limit = match p.ctx.int("dex.pool_load_retries") {
        Ok(n) => n,
        Err(e) => {
            p.signals
                .once(
                    &p.ctx,
                    Level::Error,
                    "dex.pool",
                    format!("Повтор чтения пулов невозможен: {}", e.message),
                )
                .await;
            // Без dex.pool_load_retries ждущие «застряли» — сводку первого прохода не держат (loading.rs).
            for m in missing.iter() {
                p.loading.stuck(p, &pools[m.i].0.reference, true);
            }
            return Ok(());
        }
    };
    // Решение 339: пулы на паузе после неудач (pool_fail.rs) ждут дальше, попытка не тратится; не задана — пауз нет.
    let (waiting, later): (Vec<Missing>, Vec<Missing>) = std::mem::take(missing)
        .into_iter()
        .partition(|m| !super::pool_fail::paused(p, &pools[m.i].0.reference));
    let mut keep = later;
    if waiting.is_empty() {
        *missing = keep;
        return Ok(());
    }
    let items: Vec<(&Found, &str)> = waiting
        .iter()
        .map(|m| (&pools[m.i].0, pools[m.i].1.as_str()))
        .collect();
    let results = load_many(p, &items, at).await?;
    let isolate = super::pool_fail::base(&p.ctx).is_some();
    for (mut m, res) in waiting.into_iter().zip(results) {
        let (found, ticker) = &pools[m.i];
        m.tries += 1;
        let title = found.family.title;
        // Экран «Монеты»: пул, ни разу не прочитанный, виден с причиной (pool_unread.rs, admit.rs unread).
        if let Err(why) = &res {
            super::admit::unread(p, found, why);
        }
        // Решение 339: прочая неудача — пауза растёт (журнал — только при смене причины); StaleData — без паузы.
        if let Err(why) = &res
            && !super::stale_read::is_stale(why)
        {
            super::pool_fail::note(p, &found.reference, title, ticker, why).await;
        }
        match res {
            Ok(()) => {
                let msg = format!(
                    "{title} {ticker}: состояние пула прочитано с попытки {}",
                    m.tries + 1
                );
                p.ctx.log(Level::Info, "dex.pool", msg, None).await;
                p.signals.pool_ready();
            }
            // Ревью t153 (Б1): при dex.pool_fail_pause_ms пул не бросается (кроме незаданной настройки) — повтор
            // дальше с паузой; одна запись и «недостоверен» вместо «загружается» — на переходе через предел.
            Err(why) if m.tries >= limit && why.setting.is_none() && isolate => {
                if super::stale_read::is_stale(&why) {
                    super::pool_fail::note(p, &found.reference, title, ticker, &why).await;
                }
                p.loading.done(&found.reference);
                if m.tries - 1 < limit {
                    let msg = format!(
                        "{title} {ticker} ({}): состояние пула не прочитано за {} попыток: {} — недостоверен (в \
                         котировки и сделки не идёт); повтор продолжается с паузой dex.pool_fail_pause_ms, пока не \
                         прочитается (решение 339, ревью t153)",
                        label(&found.reference),
                        m.tries + 1,
                        why.message
                    );
                    p.ctx.log(Level::Error, "dex.pool", msg, None).await;
                    p.loading.publish(p).await;
                }
                keep.push(m);
            }
            Err(why) if m.tries >= limit => {
                // Ревью 298: по факту кода брошенный пул возвращают переподключение пачки, задание НЕЗАДАННОЙ настройки
                // (только если причина — она, revive.rs), а также перечитывание по его событию и пересинхрон ленты
                // (depth_sync.rs resync) — не любое изменение настроек.
                let until = match &why.setting {
                    Some(name) => format!("до задания настройки {name} или переподключения пачки"),
                    None => "до переподключения пачки (или перечитывания по событию пула / пересинхрона ленты)".to_string(),
                };
                let msg = format!(
                    "{title} {ticker} ({}): состояние пула так и не прочитано за {} попыток: {} — недостоверен \
                     {until}",
                    label(&found.reference),
                    m.tries + 1,
                    why.message
                );
                p.ctx.log(Level::Error, "dex.pool", msg, None).await;
                // Дальше пул «недостоверен», а не «загружается» (loading.rs).
                p.loading.done(&found.reference);
                p.loading.publish(p).await;
                // Брошен из-за незаданной настройки — вернётся, когда её зададут (on_settings).
                abandoned.push(Abandoned {
                    i: m.i,
                    setting: why.setting.clone(),
                });
            }
            Err(why) => {
                // Ждёт незаданную настройку — пул «застрял», сводку первого прохода не держит (loading.rs).
                p.loading.stuck(p, &found.reference, why.setting.is_some());
                keep.push(m)
            }
        }
    }
    *missing = keep;
    Ok(())
}

/// Чтение одного пула на блоке и отправка ядру. Возвращает: Ok(Ok) — прочитан; Ok(Err(причина)) — нет.
/// Ошибки: End — ядро закрыло канал.
pub async fn load_one(
    p: &Pools,
    found: &Found,
    ticker: &str,
    block: u64,
) -> Result<Result<(), GatewayError>, End> {
    load_one_with(p, found, ticker, block, false).await
}

/// Что делает: как load_one, но для чтения «каждым блоком» (per_block*.rs): при dex.per_block_skip_same неизменившееся
/// состояние ядру не шлётся (задача t160, шапка). Принимает и возвращает — как load_one. Ошибки: End — ядро закрыло канал.
pub async fn load_one_block(
    p: &Pools,
    found: &Found,
    ticker: &str,
    block: u64,
) -> Result<Result<(), GatewayError>, End> {
    load_one_with(p, found, ticker, block, skip_same(p)).await
}

/// Чтение одного пула на блоке; `skip` — не слать ядру, если поколение не изменилось. Ошибки: End — ядро закрыло канал.
async fn load_one_with(
    p: &Pools,
    found: &Found,
    ticker: &str,
    block: u64,
    skip: bool,
) -> Result<Result<(), GatewayError>, End> {
    let res = match p.reg.get(&found.reference) {
        Some(e) => load::refresh(&p.ctx, &p.tokens, &p.reg, &e, block).await,
        None => {
            load::entry(
                &p.ctx,
                &p.tokens,
                &p.reg,
                found,
                ticker,
                listen_log::found_usdt(found),
                block,
            )
            .await
        }
    };
    match res {
        Ok(e) => put_emit(p, e, skip).await.map(Ok),
        Err(e) => Ok(Err(e)),
    }
}

/// Что делает: задача t160 — пропускать ли неизменившееся состояние пула «каждым блоком»: dex.per_block_skip_same = да
/// и dex.pool_events_keep = да (событие при полном канале не теряется). Принимает: окружение пачки. Ошибок нет.
pub fn skip_same(p: &Pools) -> bool {
    // Сначала настройка keep в памяти (без чтения хранилища): выключена — dex.per_block_skip_same не читается.
    p.kept.keep.on() && p.ctx.flag(SKIP_SAME).unwrap_or(false)
}

/// Что делает: запись прочитанного пула в реестр. `skip` = false — ровно put (как раньше), true; иначе — true, если
/// поколение состояния сменилось (первое чтение, возврат из недостоверного, новое состояние), false — то же и не 0.
/// Принимает: окружение пачки, запись, skip. Возвращает: слать ли пул ядру. Ошибок нет.
pub fn put_new(p: &Pools, e: Entry, skip: bool) -> bool {
    if !skip {
        return p.reg.put_observed(e);
    }
    let r = e.pool.reference.clone();
    let before = p.reg.entry(&r).map(|x| x.state_gen);
    if !p.reg.put_observed(e) {return false;}
    let after = p.reg.entry(&r).map(|x| x.state_gen);
    !matches!((before, after), (Some(b), Some(a)) if a == b && a != 0)
}

/// Что делает: прочитанный пул — в реестр, из «загружается», ядру (как раньше load_one; при `skip` — только если
/// поколение сменилось, put_new). Принимает: окружение пачки, запись, skip. Ошибки: End — ядро закрыло канал.
pub async fn put_emit(p: &Pools, e: Entry, skip: bool) -> Result<(), End> {
    listen_log::note_refill(p, &e);
    let r = e.pool.reference.clone();
    let pool = e.pool.clone();
    let fresh = put_new(p, e, skip);
    p.loading.done(&r);
    p.loading.publish(p).await;
    if fresh {
        listen_log::emit(p, pool)
    } else {
        Ok(())
    }
}

pub use super::reload_many::load_many;
