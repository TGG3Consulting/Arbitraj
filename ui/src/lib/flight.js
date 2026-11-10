// Файл: ui/src/lib/flight.js
// Что делает: «один запрос в полёте» (single-flight) для перечитывания экрана или хранилища. Факт 10.11.2026: живой
//   интерфейс дал 893 ошибки net::ERR_INSUFFICIENT_RESOURCES — перечитывание стартовало на каждое живое сообщение,
//   не дожидаясь прежнего, и браузер исчерпал лимит одновременных запросов.
//   Пока перечитывание идёт, новый вызов не стартует второй запрос, а помечает «нужно ещё раз»: после окончания —
//   ровно один повтор (данные свежее последнего вызова, запросов в полёте — не больше одного).
//   cancel() — уход со страницы или смена параметров: текущий запрос отменяется (AbortController), повтор снимается;
//   следующий вызов начинает заново сразу.
// Кто вызывает: lib/coinNames.svelte.js, lib/remote.svelte.js, lib/dex.svelte.js, lib/delist.svelte.js, хранилища
//   lib/*.svelte.js и экраны с перечитыванием по живому потоку (pages/*, components/*).
// От чего зависит: AbortController браузера; ошибка отмены — lib/api.js (поле aborted).
// Настройки: нет.

/**
 * Что делает: оборачивает функцию перечитывания в single-flight.
 * Принимает: fn(signal) — async-функция; signal передаёт в get(path, signal) (lib/api.js). Первый вызов fn идёт
 *   синхронно внутри вызова run() — чтения $state до первого await отслеживаются $effect, как без обёртки.
 * Возвращает: run() — запустить или пометить повтор; его Promise завершается после всех повторов этого полёта
 *   (вызов во время полёта получает тот же Promise); run.cancel() — отменить текущий запрос и повтор.
 * Ошибки: ошибка отмены (aborted) глотается; прочие ошибки fn уходят выше как есть (fn обычно ловит их сама).
 */
export function flight(fn) {
  let ctrl = null;
  let again = false;
  let done = Promise.resolve();

  /** Полёт: fn и повторы, пока их просят. Принимает: свой AbortController. */
  async function fly(my) {
    try {
      do {
        again = false;
        await fn(my.signal);
      } while (again && ctrl === my && !my.signal.aborted);
    } catch (e) {
      if (!e?.aborted) throw e;
    } finally {
      if (ctrl === my) ctrl = null;
    }
  }

  function run() {
    if (ctrl) {
      again = true;
      return done;
    }
    ctrl = new AbortController();
    done = fly(ctrl);
    return done;
  }

  run.cancel = () => {
    again = false;
    if (ctrl) {
      const c = ctrl;
      ctrl = null;
      c.abort();
    }
  };
  return run;
}
