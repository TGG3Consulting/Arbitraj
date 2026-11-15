// Файл: contracts/evmtest/izi.mjs
// Что делает: решение 286 — шаг IZI (вид 14) ArbExecutor на EVM ethereumjs (Cancun) с пулом-фикстурой
//   fixtures/IziFixture.sol (подписи swapX2Y / swapY2X и оплата в swapX2YCallback / swapY2XCallback, как у iZiSwap):
//   I1) v2: execute в обе стороны (USDT → COIN и COIN → USDT) шагом 14 — направление по адресам (tokenIn < tokenOut →
//       swapX2Y), владелец получил выход пула, остаток входа (фикстура берёт сумму − 1 wei) вернулся владельцу, на исполнителе пусто;
//   I2) посторонний зовёт swapX2YCallback / swapY2XCallback исполнителя — откат «pool»;
//   I3) исполнитель v1 — шаг 14 откатывается «kind».
//   Проба на настоящих пулах трёх фабрик на цепи — tests/data/izi_2026-11-15/exec_sim.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/IziFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must } = L;
const { E18, OWNER, LP, OTHER, ERC20, v1, v2, world, exe, bal, X } = W;
const IZI = L.compile(L.solc08, join(L.here, 'fixtures'), 'IziFixture.sol', 'IziFixture', W.opt);
const step = (pool, tin, tout) => [14, pool, tin, tout, 10000, '0x'];

/// Пул-фикстура с 10 000 : 10 000 от LP. Возвращает адрес пула.
async function iziPool(c, usdt, coin) {
  const p = await L.deploy(c, LP, IZI, ['address', 'address'], [usdt, coin]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [p, 10_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [p, 10_000n * E18]);
  return p;
}

/// Сценарии I1–I3. Ошибок нет: провал — check бросает.
export async function izi() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const p = await iziPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 100n * E18;
    for (const [tin, tout] of [[usdt, coin], [coin, usdt]]) {
      const x2y = BigInt(tin) < BigInt(tout);
      const inBefore = await bal(c, tin, OWNER);
      const outBefore = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, amt - 1n, [step(p, tin, tout)]));
      check(r.ok, `IZI: execute шагом 14 прошёл (${x2y ? 'swapX2Y' : 'swapY2X'}, газ ${r.gas})`);
      check((await bal(c, tout, OWNER)) - outBefore === amt - 1n, 'IZI: владелец получил выход пула');
      check(inBefore - (await bal(c, tin, OWNER)) === amt - 1n, 'IZI: остаток входа (1 wei, пул взял меньше) вернулся владельцу');
      check((await bal(c, tin, ex)) === 0n && (await bal(c, tout, ex)) === 0n, 'IZI: на исполнителе ничего не осталось');
    }
    for (const fn of ['swapX2YCallback', 'swapY2XCallback']) {
      const s = await L.call(c, OTHER, ex, X.encodeFunctionData(fn, [amt, amt, '0x']));
      check(!s.ok && s.reason === 'pool', `IZI: посторонний зовёт ${fn} — откат «pool»`);
    }
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const p = await iziPool(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(p, usdt, coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг IZI (14) — откат «kind» (вида нет в v1)');
  }
}
