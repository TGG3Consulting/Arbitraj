//! Файл: venues/bsc_dex/pools/indexer_gap.rs
//! Что делает: ревью t251 (решение 482) — есть ли у DEX справочник индексатора. Факт конфигурации, без новой настройки:
//! список DEX сети у индексатора (GeckoTerminal GET /networks/{net}/dexes — directory/fetch.rs dexes, решение 58;
//! хранится в SQLite и восстанавливается при старте — directory/refresh.rs restore) против id DEX области «DEX» (ключ
//! — id индексатора, у сети не по умолчанию — с «@сеть», решение 435). Источники пулов решения 482 (поиск
//! getAllLBPairs, подписка LBPairCreated, перечисление фабрики Metric) включаются только у DEX, которых в этом списке
//! нет: у DEX из списка пулы, как раньше, даёт индексатор — поведение площадки BSC (живой бот) не меняется.
//! Кто вызывает: pools/factory_calls.rs, pools/created_log.rs, pools/metric_list.rs.
//! От чего зависит: ctx.rs (index — справочник), directory/mod.rs (Snap.dexes, dexes_at).
//! Настройки: нет.

use super::super::ctx::Ctx;

/// id индексатора из ключа DEX: часть до «@» (ключ сети по умолчанию — id как есть). Ошибок нет.
pub fn indexer_id(key: &str) -> &str {
    key.split('@').next().unwrap_or(key)
}

/// Нет ли DEX в списке индексатора. Принимает: окружение, ключ DEX. Возвращает: Some(true) — список получен, DEX в нём
/// нет (пулы — только своими источниками); Some(false) — DEX в списке (пулы даёт индексатор); None — список DEX сети
/// ещё не получен (источник не включается: не знаем — не меняем поведение). Ошибок нет.
pub fn outside(ctx: &Ctx, key: &str) -> Option<bool> {
    let bindings = crate::settings::indexer_id::Bindings::read(&ctx.settings);
    let external = bindings.source(key);
    ctx.index.with(|s| {
        s.dexes_at.map(|_| {
            !s.dexes.iter().any(|d| {
                Some(d.id.as_str()) == external
                    || (external == Some(key) && d.id == indexer_id(key))
            })
        })
    })
}

/// Текст заметки, почему источник решения 482 у DEX не включён. Принимает: ключ DEX, итог outside. Ошибок нет.
pub fn why_not(key: &str, got: Option<bool>) -> String {
    let why = match got {
        Some(false) => "пулы даёт индексатор — свой источник не нужен",
        _ => "список DEX индексатора не получен — свой источник не включён",
    };
    format!("{key}: {why}")
}

#[cfg(test)]
mod tests {
    use super::indexer_id;

    /// Ключ сети по умолчанию — id как есть, другой сети — часть до «@».
    #[test]
    fn id_before_at() {
        assert_eq!(indexer_id("metric-bsc"), "metric-bsc");
        assert_eq!(indexer_id("swapline-v1@arbitrum"), "swapline-v1");
    }
}
