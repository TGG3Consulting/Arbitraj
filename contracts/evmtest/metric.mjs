// Файл: contracts/evmtest/metric.mjs
// Что делает: решение 285 — шаг METRIC (вид 13) ArbExecutor на EVM ethereumjs (Cancun) с пулом-фикстурой
//   fixtures/MetricFixture.sol (подпись swap и оплата в metricOmmSwapCallback, как у пулов Metric BSC):
//   M1) v2: execute USDT → COIN шагом 13 (вход — с владельца через исполнитель, оплата в обратном вызове), владелец
//       получил выход пула, на исполнителе пусто; порядок токенов пула «не по адресу» — флаг zeroForOne из data;
//   M2) неверный флаг zeroForOne — откат (пул спросил другой токен, _pay платит только tokenIn шага);
//   M3) посторонний зовёт metricOmmSwapCallback исполнителя — откат «pool»;
//   M4) исполнитель v1 — шаг 13 откатывается «kind».
//   Проба на настоящем пуле на цепи — tests/data/metric_2026-11-15/exec_sim.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/MetricFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must, coder } = L;
const { E18, OWNER, LP, OTHER, ERC20, v1, v2, world, exe, bal, X } = W;
const MET = L.compile(L.solc08, join(L.here, 'fixtures'), 'MetricFixture.sol', 'MetricFixture', W.opt);
const step = (pool, tin, tout, zf) => [13, pool, tin, tout, 10000, coder.encode(['bool'], [zf])];

/// Пул-фикстура (token0 = большее из адресов — «не по адресу») с 10 000 : 10 000 от LP. Возвращает (пул, token0).
async function metricPool(c, usdt, coin) {
  const [t0, t1] = usdt.toLowerCase() > coin.toLowerCase() ? [usdt, coin] : [coin, usdt];
  const p = await L.deploy(c, LP, MET, ['address', 'address'], [t0, t1]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [p, 10_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [p, 10_000n * E18]);
  return { p, t0 };
}

/// Сценарии M1–M4. Ошибок нет: провал — check бросает.
export async function metric() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const { p, t0 } = await metricPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const zf = t0.toLowerCase() === usdt.toLowerCase();
    const amt = 100n * E18;
    const before = await bal(c, coin, OWNER);
    const r = await L.call(c, OWNER, ex, exe(usdt, amt, coin, amt, [step(p, usdt, coin, zf)]));
    check(r.ok, `METRIC: execute USDT → COIN шагом 13 прошёл (газ ${r.gas}; token0 пула — больший адрес, zeroForOne = ${zf})`);
    check((await bal(c, coin, OWNER)) - before === amt, 'METRIC: владелец получил выход пула (1:1)');
    check((await bal(c, usdt, ex)) === 0n && (await bal(c, coin, ex)) === 0n, 'METRIC: на исполнителе ничего не осталось');
    const w = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, [step(p, usdt, coin, !zf)]));
    check(!w.ok, `METRIC: неверный флаг zeroForOne — откат (${w.reason})`);
    const cb = X.encodeFunctionData('metricOmmSwapCallback', [amt, -amt, '0x']);
    const s = await L.call(c, OTHER, ex, cb);
    check(!s.ok && s.reason === 'pool', 'METRIC: посторонний зовёт metricOmmSwapCallback — откат «pool»');
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const { p, t0 } = await metricPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const zf = t0.toLowerCase() === usdt.toLowerCase();
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(p, usdt, coin, zf)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг METRIC (13) — откат «kind» (вида нет в v1)');
  }
}
