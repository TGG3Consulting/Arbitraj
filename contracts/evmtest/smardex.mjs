// Файл: contracts/evmtest/smardex.mjs
// Что делает: решение 288 — шаг SMARDEX (вид 16) ArbExecutor на EVM ethereumjs (Cancun) с парой-фикстурой
//   fixtures/SmardexFixture.sol (подпись swap и оплата в smardexSwapCallback, как у SmardexPair):
//   S1) v2: execute USDT → COIN шагом 16 (вход — с владельца через исполнитель, оплата в обратном вызове) в обе стороны
//       токенов (zeroForOne по порядку адресов), владелец получил выход пары, на исполнителе пусто; minOut + 1 — откат;
//   S2) посторонний зовёт smardexSwapCallback исполнителя — откат «pool»;
//   S3) исполнитель v1 — шаг 16 откатывается «kind».
//   Проба на настоящих парах на цепи — tests/data/smardex_2026-11-15/exec_sim.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/SmardexFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must } = L;
const { E18, OWNER, LP, OTHER, ERC20, v1, v2, world, exe, bal, X } = W;
const SDX = L.compile(L.solc08, join(L.here, 'fixtures'), 'SmardexFixture.sol', 'SmardexFixture', W.opt);
const step = (pool, tin, tout) => [16, pool, tin, tout, 10000, '0x'];

/// Пара-фикстура USDT/COIN с 10 000 : 10 000 от LP. Возвращает адрес.
async function sdxPair(c, usdt, coin) {
  const p = await L.deploy(c, LP, SDX, ['address', 'address'], [usdt, coin]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [p, 10_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [p, 10_000n * E18]);
  return p;
}

/// Сценарии S1–S3. Ошибок нет: провал — check бросает.
export async function smardex() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const p = await sdxPair(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 100n * E18;
    const exp = amt * 99n / 100n;
    const bad = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp + 1n, [step(p, usdt, coin)]));
    check(!bad.ok && bad.reason === 'minOut', 'SMARDEX: minOut + 1 — откат «minOut»');
    for (const [tin, tout, name] of [[usdt, coin, 'USDT → COIN'], [coin, usdt, 'COIN → USDT']]) {
      const before = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, exp, [step(p, tin, tout)]));
      check(r.ok, `SMARDEX: execute ${name} шагом 16 прошёл (газ ${r.gas}; zeroForOne = ${BigInt(tin) < BigInt(tout)})`);
      check((await bal(c, tout, OWNER)) - before === exp, `SMARDEX: ${name} — владелец получил выход пары`);
      check((await bal(c, tin, ex)) === 0n && (await bal(c, tout, ex)) === 0n, `SMARDEX: ${name} — на исполнителе ничего не осталось`);
    }
    const cb = X.encodeFunctionData('smardexSwapCallback', [amt, -amt, '0x']);
    const s = await L.call(c, OTHER, ex, cb);
    check(!s.ok && s.reason === 'pool', 'SMARDEX: посторонний зовёт smardexSwapCallback — откат «pool»');
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const p = await sdxPair(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(p, usdt, coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг SMARDEX (16) — откат «kind» (вида нет в v1)');
  }
}
