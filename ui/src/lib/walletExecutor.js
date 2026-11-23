import { get, seg } from './api.js';

// Версии из сверки кода на узле. Неизвестная версия не подтверждает поддержку.
export function executorFeatures(row) {
  const singleton = ['v3', 'v4', 'v5'].includes(row.executor_version);
  const native = row.executor_version === 'v2' || singleton;
  const nativeOk = native && !!row.executor_wrapped && !!row.native_wrapped
    && row.executor_wrapped.toLowerCase() === row.native_wrapped.toLowerCase();
  return { singleton, native, nativeOk };
}

// Подтверждение относится к кошельку, сети и прежнему адресу на момент запроса оценки.
export async function requestExecutorQuote(venue, wallet, executor, read = get) {
  const path = `/api/venues/${seg(venue)}/executor`;
  const query = `?wallet=${seg(wallet)}`;
  const quote = await read(`${path}/quote${query}`);
  return { ...quote, deployUrl: `${path}/deploy${query}`, previousExecutor: executor };
}
