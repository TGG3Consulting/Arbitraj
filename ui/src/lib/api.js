// Файл: ui/src/lib/api.js
// Что делает: запросы к REST бэкенда (src/api). Ошибка бэкенда превращается в Error с его текстом как есть:
//   JSON {"error": "..."} → текст error; простой текст (например отказ api/guard.rs) → этот текст.
//   У ошибки есть поле status — HTTP-код (0 — запрос не дошёл): по нему экраны отличают
//   «не применимо» (400), «площадка не подключена» (404), «нет данных» (503) — src/api/README.md.
//   Отмена (AbortController, lib/flight.js): ошибка с полем aborted = true — экран её не показывает.
//   GET одного адреса (факт 10.11.2026: 893 ошибки net::ERR_INSUFFICIENT_RESOURCES — браузер исчерпал лимит
//   одновременных запросов): в полёте не больше одного запроса на адрес. Вызов, пока запрос идёт, не стартует второй
//   и не берёт ответ, начатый до него, а ждёт следующий запрос, который уйдёт сразу после текущего (все такие
//   вызовы — один общий запрос). Ответ разбирается отдельно для каждого вызова (свои объекты, не общие).
// Кто вызывает: все экраны и хранилища ui/src/lib/*.svelte.js; download — pages/Uncapped.svelte (решение 215).
// От чего зависит: fetch и AbortController браузера.
// Настройки: нет. Адрес — тот же сервер, что отдал страницу (или proxy vite в разработке).

/** Ошибка отмены запроса (ушли со страницы, сменили параметры). Возвращает: Error с aborted = true, status 0. */
const abortError = () => Object.assign(new Error('Запрос отменён'), { aborted: true, status: 0 });

/**
 * Что делает: отправляет запрос и читает тело текстом.
 * Принимает: метод, путь, тело (объект → JSON) или undefined, AbortSignal или undefined.
 * Возвращает: {ok, status, statusText, text}.
 * Ошибки: «Нет связи с сервером: …» (status 0); отмена — abortError.
 */
async function send(method, path, body, signal) {
  const init = { method, headers: {}, signal };
  if (body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(body);
  }
  try {
    const res = await fetch(path, init);
    return { ok: res.ok, status: res.status, statusText: res.statusText, text: await res.text() };
  } catch (e) {
    if (e?.name === 'AbortError') throw abortError();
    throw Object.assign(new Error(`Нет связи с сервером: ${e.message}`), { status: 0 });
  }
}

/**
 * Что делает: разбирает ответ send. Принимает: {ok, status, statusText, text}.
 * Возвращает: разобранный JSON (или текст; null для пустого ответа). Ошибки: Error с текстом бэкенда и status.
 */
function parse(r) {
  let data = null;
  if (r.text) {
    try {
      data = JSON.parse(r.text);
    } catch {
      data = r.text;
    }
  }
  if (!r.ok) {
    const reason = data && typeof data === 'object' && 'error' in data ? data.error : data;
    throw Object.assign(new Error(reason || `Ошибка ${r.status} ${r.statusText}`), { status: r.status });
  }
  return data;
}

/**
 * Что делает: отправляет запрос и разбирает ответ.
 * Принимает: метод, путь (/api/...), тело (объект → JSON) или undefined, AbortSignal (необязательно).
 * Возвращает: разобранный JSON ответа (или null для пустого ответа).
 * Ошибки: Error с текстом бэкенда и status; Error «Нет связи с сервером: …» (status 0), если запрос не дошёл;
 *   отмена — Error с aborted = true.
 */
export async function request(method, path, body, signal) {
  return parse(await send(method, path, body, signal));
}

/** GET в полёте по адресу: cur — идёт, next — общий следующий (уйдёт после cur). */
const flights = new Map();

/** Общий запрос: ctrl — отмена; users — вызовы с AbortSignal; pinned — есть вызов без отмены; done — ответ send. */
function entry() {
  const e = { ctrl: new AbortController(), users: 0, pinned: false };
  e.done = new Promise((res, rej) => Object.assign(e, { res, rej }));
  e.done.catch(() => {}); // все вызовы отменились — ответ никому не нужен, без «unhandled rejection» в консоли.
  return e;
}

/** Отправляет общий запрос адреса; по окончании — следующий или освобождает адрес. Ошибок нет (они — в e.done). */
function launch(path, f, e) {
  const next = () => {
    f.cur = f.next;
    f.next = null;
    if (f.cur) launch(path, f, f.cur);
    else flights.delete(path);
  };
  if (e.ctrl.signal.aborted) {
    e.rej(abortError());
    next();
    return;
  }
  send('GET', path, undefined, e.ctrl.signal).then(e.res, e.rej).finally(next);
}

/**
 * Что делает: привязывает вызов к общему запросу. Принимает: общий запрос, AbortSignal или undefined.
 * Возвращает: разобранный ответ. Ошибки: как request; отмена своего signal — abortError (общий запрос
 *   отменяется, только когда отменились все вызовы с signal и нет вызова без него).
 */
function attach(e, signal) {
  if (!signal) {
    e.pinned = true;
    return e.done.then(parse);
  }
  if (signal.aborted) return Promise.reject(abortError());
  e.users++;
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      e.users--;
      if (!e.pinned && e.users === 0) e.ctrl.abort();
      reject(abortError());
    };
    signal.addEventListener('abort', onAbort, { once: true });
    e.done.then(
      (r) => {
        signal.removeEventListener('abort', onAbort);
        try {
          resolve(parse(r));
        } catch (err) {
          reject(err);
        }
      },
      (err) => {
        signal.removeEventListener('abort', onAbort);
        reject(err);
      },
    );
  });
}

/**
 * GET: не больше одного запроса адреса в полёте (см. шапку). Принимает: путь, AbortSignal (необязательно).
 * Возвращает: JSON. Ошибки: как request.
 */
export function get(path, signal) {
  if (signal?.aborted) return Promise.reject(abortError());
  let f = flights.get(path);
  if (!f) {
    f = { cur: null, next: null };
    flights.set(path, f);
  }
  if (!f.cur) {
    f.cur = entry();
    const e = f.cur;
    const p = attach(e, signal);
    launch(path, f, e);
    return p;
  }
  f.next ??= entry();
  return attach(f.next, signal);
}

/** POST. Принимает: путь, тело. Возвращает: JSON. Ошибки: как request. */
export const post = (path, body) => request('POST', path, body);
/** PUT. Принимает: путь, тело. Возвращает: JSON. Ошибки: как request. */
export const put = (path, body) => request('PUT', path, body);
/** DELETE с телом. Принимает: путь, тело. Возвращает: JSON. Ошибки: как request. */
export const del = (path, body) => request('DELETE', path, body);

/** Кодирует часть пути (имя секрета, id площадки). Принимает: строку. Возвращает: строку. */
export const seg = (s) => encodeURIComponent(s);

/**
 * Что делает: скачивает файл ответа (решение 215: отчёт Excel «Упущено») — сохраняет под именем из Content-Disposition.
 * Принимает: путь (/api/...), запасное имя файла. Возвращает: ничего.
 * Ошибки: Error с текстом бэкенда и status (как request); «Нет связи с сервером: …» (status 0).
 */
export async function download(path, fallbackName) {
  let res;
  try {
    res = await fetch(path);
  } catch (e) {
    throw Object.assign(new Error(`Нет связи с сервером: ${e.message}`), { status: 0 });
  }
  if (!res.ok) {
    const text = await res.text();
    let reason = text;
    try {
      reason = JSON.parse(text).error ?? text;
    } catch {
      // Простой текст — как есть.
    }
    throw Object.assign(new Error(reason || `Ошибка ${res.status} ${res.statusText}`), { status: res.status });
  }
  const blob = await res.blob();
  const name = /filename="([^"]+)"/.exec(res.headers.get('Content-Disposition') ?? '')?.[1] ?? fallbackName;
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}
