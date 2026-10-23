// Файл: ui/src/lib/router.svelte.js
// Что делает: простая навигация по адресу после «#»: «#/settings», «#/unlock/telegram.bot_token».
//   Первая часть — экран, вторая (необязательная) — на что перейти внутри экрана.
// Кто вызывает: App.svelte (какой экран показать), экраны (ссылки и переход к элементу).
// От чего зависит: location.hash браузера.
// Настройки: нет.

/** Текущий экран и его аргумент. */
export const route = $state(parse());

/** Разбирает location.hash. Возвращает: {page, arg}; пустой адрес — page ''. */
function parse() {
  const [page = '', ...rest] = location.hash.replace(/^#\/?/, '').split('/');
  return { page: decodeURIComponent(page), arg: decodeURIComponent(rest.join('/')) };
}

window.addEventListener('hashchange', () => Object.assign(route, parse()));

/** Адрес ссылки. Принимает: экран и необязательный аргумент. Возвращает: строку «#/…». */
export const href = (page, arg) => `#/${page}${arg ? `/${encodeURIComponent(arg)}` : ''}`;
