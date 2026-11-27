//! Допуск новой отправки цикла, включая approve. Наблюдение уже переданной операции выполняется отдельно.
use crate::gateway::errors::GatewayResult;
use std::future::Future;

tokio::task_local! {
    static CHECK: Box<dyn Fn() -> GatewayResult<()> + Send + Sync>;
}

pub async fn scope<T>(
    check: impl Fn() -> GatewayResult<()> + Send + Sync + 'static,
    work: impl Future<Output = T>,
) -> T {
    CHECK.scope(Box::new(check), work).await
}

pub fn active() -> bool {
    CHECK.try_with(|_| ()).is_ok()
}

pub fn before_post() -> GatewayResult<()> {
    CHECK.try_with(|check| check()).unwrap_or(Ok(()))
}
