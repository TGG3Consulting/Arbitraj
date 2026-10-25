//! Файл: venues/bsc_dex/check/fresh.rs
//! Что делает: какие пулы цикла проверок (сверка с квотером, крючки) ещё ни разу не проверены этим циклом — их
//! проверяют сразу, как только пул стал достоверным (прочитан при старте, перечитан, принят новым), а не через
//! интервал полного прохода (решение 65: пулы, дочитанные после старта цикла, ждали dex.quoter_check_s — 15 минут
//! без цен). И когда первый проход окончен: ни один пул не читается (pools/loading.rs reading — застрявшие из-за
//! незаданной настройки не в счёт, как у налогов, решение 63) и все достоверные пулы проверены.
//! Кто вызывает: check/quoter_cycle.rs, check/cycle.rs.
//! От чего зависит: pools/listen.rs (Pools: реестр, «загружаются»).
//! Настройки: нет.

use std::collections::HashSet;

use crate::core::types::PoolRef;

use super::super::pools::listen::Pools;

/// Пулы, уже проверенные циклом хотя бы раз (любой итог, в том числе «мельче порога» — проверять нечего).
#[derive(Default)]
pub struct Fresh {
    seen: HashSet<PoolRef>,
}

impl Fresh {
    /// Достоверные пулы реестра, которые надо проверить сейчас: ещё не проверенные циклом и те, для которых `again`
    /// (например, сверка ждала влияния крючка, а оно уже измерено). Принимает: окружение, условие повтора.
    /// Возвращает: пулы. Ошибок нет.
    pub fn due(&self, p: &Pools, again: impl Fn(&PoolRef) -> bool) -> Vec<PoolRef> {
        p.reg
            .refs()
            .into_iter()
            .filter(|r| p.reg.update(r, |e| e.valid).unwrap_or(false))
            .filter(|r| !self.seen.contains(r) || again(r))
            .collect()
    }

    /// Пул проверен циклом. Ошибок нет.
    pub fn mark(&mut self, r: &PoolRef) {
        self.seen.insert(r.clone());
    }

    /// Первый проход окончен: ни один пул не читается и все достоверные пулы проверены. Ошибок нет.
    pub fn settled(&self, p: &Pools) -> bool {
        !p.loading.reading(p) && self.due(p, |_| false).is_empty()
    }
}
