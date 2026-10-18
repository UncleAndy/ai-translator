// Прогон логики страницы большого экрана под node с заглушкой DOM.
//
// Запуск:
//   nix shell nixpkgs#nodejs -c node web/test/page.mjs
//
// Проверяет обработку событий перевода: ревизии одной фразы, финал, удержание
// последних трёх фраз, отбрасывание повторов, состояния линии и обрыв связи.
// Про new Function: в тело подставляется только исходник нашего же файла
// web/index.html, внешних данных в подстановке нет.

import { readFileSync } from "node:fs";

const PAGE = new URL("../index.html", import.meta.url);
const html = readFileSync(PAGE, "utf8");
const script = html.match(/<script>([\s\S]*?)<\/script>/);
if (!script) {
  console.error("в web/index.html не найден блок <script>");
  process.exit(1);
}
const SRC = script[1];

class ClassList {
  constructor(el) {
    this.el = el;
  }
  has(name) {
    return this.el.className.split(" ").filter(Boolean).includes(name);
  }
  add(name) {
    if (!this.has(name)) this.el.className = (this.el.className + " " + name).trim();
  }
  remove(name) {
    this.el.className = this.el.className.split(" ").filter((c) => c && c !== name).join(" ");
  }
  toggle(name, on) {
    if (on) this.add(name);
    else this.remove(name);
  }
}

class El {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.className = "";
    this.textContent = "";
    this.parent = null;
    this.classList = new ClassList(this);
    this.lastSize = null;
    const self = this;
    this.style = {
      setProperty(name, value) {
        self.lastSize = value;
      },
      cursor: "",
    };
  }
  get firstChild() {
    return this.children[0] ?? null;
  }
  get scrollHeight() {
    return this.children.length * 100;
  }
  get clientHeight() {
    return 400;
  }
  appendChild(child) {
    child.parent = this;
    this.children.push(child);
    return child;
  }
  insertBefore(child, ref) {
    child.parent = this;
    const index = this.children.indexOf(ref);
    this.children.splice(index < 0 ? 0 : index, 0, child);
    return child;
  }
  removeChild(child) {
    const index = this.children.indexOf(child);
    if (index >= 0) this.children.splice(index, 1);
    return child;
  }
  querySelector(selector) {
    const want = selector.replace(".", "");
    for (const child of this.children) {
      if (child.className.split(" ").filter(Boolean).includes(want)) return child;
    }
    return null;
  }
}

function load(protocol, stageHeight = 900) {
  const stageEl = new El("main");
  const linesEl = new El("div");
  const statusEl = new El("div");
  const documentElement = new El("html");
  const body = new El("body");

  // Высота текста в заглушке пропорциональна размеру шрифта — иначе подбор
  // размера (fit) нечем проверить. Множитель произвольный: важны отношения.
  let lineSizeVw = 4.5;
  let stage = stageHeight;

  documentElement.style.setProperty = (_name, value) => {
    lineSizeVw = parseFloat(value);
  };
  Object.defineProperty(linesEl, "scrollHeight", {
    get: () => linesEl.children.length * lineSizeVw * 20,
  });
  Object.defineProperty(stageEl, "clientHeight", {
    get: () => stage,
  });

  const document = {
    getElementById: (id) => ({ lines: linesEl, stage: stageEl }[id] ?? statusEl),
    createElement: (tag) => new El(tag),
    addEventListener: () => {},
    documentElement,
    body,
  };

  const timers = [];
  const location = { protocol, host: "127.0.0.1:8642" };
  const window = { addEventListener: () => {} };

  let socket = null;
  class FakeWebSocket {
    constructor(url) {
      this.url = url;
      socket = this;
    }
  }

  const run = new Function(
    "document",
    "location",
    "WebSocket",
    "console",
    "setTimeout",
    "clearTimeout",
    "window",
    SRC,
  );
  run(
    document,
    location,
    FakeWebSocket,
    { warn: () => {}, log: () => {} },
    (fn, ms) => {
      timers.push({ fn, ms });
      return timers.length;
    },
    () => {},
    window,
  );

  return {
    linesEl,
    stageEl,
    statusEl,
    documentElement,
    timers,
    socket,
    lineSize: () => lineSizeVw,
    setStageHeight(value) {
      stage = value;
    },
    send(message) {
      socket.onmessage({ data: JSON.stringify(message) });
    },
    open() {
      socket.onopen();
    },
    close() {
      socket.onclose();
    },
  };
}

let failures = 0;
function check(name, condition, detail = "") {
  if (condition) {
    console.log("ok   " + name);
  } else {
    failures += 1;
    console.log("FAIL " + name + (detail ? " — " + detail : ""));
  }
}

const phraseText = (lines, index) => lines.children[index].querySelector(".line").textContent;

// --- адрес подключения -------------------------------------------------------
{
  const page = load("http:");
  check("ws-адрес при http", page.socket.url === "ws://127.0.0.1:8642/ws", page.socket.url);

  const secure = load("https:");
  check("wss-адрес при https", secure.socket.url === "wss://127.0.0.1:8642/ws", secure.socket.url);
}

// --- ревизии одной фразы -----------------------------------------------------
{
  const page = load("http:");
  page.open();
  check("статус после подключения", page.statusEl.textContent === "на связи", page.statusEl.textContent);
  check("подключение сняло флаг обрыва", !page.statusEl.classList.has("off"));

  page.send({ line: 0, seq: 1, text: "при", is_final: false });
  page.send({ line: 0, seq: 2, text: "привет", is_final: false });
  page.send({ line: 0, seq: 3, text: "привет мир", is_final: false, source_text: "hello world" });

  check("внутри фразы одна строка", page.linesEl.children.length === 1, String(page.linesEl.children.length));
  check("текст обновляется на месте", phraseText(page.linesEl, 0) === "привет мир", phraseText(page.linesEl, 0));
  check("фраза активна", page.linesEl.children[0].classList.has("active"));

  const phrase = page.linesEl.children[0];
  const source = phrase.querySelector(".source");
  check("исходный текст показан", source !== null && source.textContent === "hello world");
  check("исходный текст выше перевода", phrase.children[0] === source);
}

// --- финал фразы -------------------------------------------------------------
{
  const page = load("http:");
  page.send({ line: 0, seq: 1, text: "при", is_final: false });
  page.send({ line: 0, seq: 2, text: "привет мир", is_final: true });

  check("финальный текст остаётся в своей фразе", phraseText(page.linesEl, 0) === "привет мир", phraseText(page.linesEl, 0));
  check("завершённая фраза приглушена", page.linesEl.children[0].classList.has("done"));
  check("завершённая фраза не активна", !page.linesEl.children[0].classList.has("active"));

  page.send({ line: 0, seq: 3, text: "следующая", is_final: false });
  check("после финала начата новая фраза", page.linesEl.children.length === 2, String(page.linesEl.children.length));
  check("новая фраза активна", page.linesEl.children[1].classList.has("active"));
  check("текст новой фразы", phraseText(page.linesEl, 1) === "следующая");
}

// --- удержание последних трёх фраз -------------------------------------------
{
  const page = load("http:");
  for (let i = 1; i <= 5; i += 1) {
    page.send({ line: 0, seq: i, text: "фраза " + i, is_final: true });
  }
  check("на экране не больше трёх фраз", page.linesEl.children.length === 3, String(page.linesEl.children.length));
  check("остались последние фразы", phraseText(page.linesEl, 2) === "фраза 5", phraseText(page.linesEl, 2));
}

// --- повторы после переподключения -------------------------------------------
{
  const page = load("http:");
  page.send({ line: 0, seq: 5, text: "пятая", is_final: false });
  page.send({ line: 0, seq: 5, text: "повтор", is_final: false });
  page.send({ line: 0, seq: 4, text: "старее", is_final: false });

  check("повтор не переписывает текст", phraseText(page.linesEl, 0) === "пятая", phraseText(page.linesEl, 0));
  check("повтор не создал фразу", page.linesEl.children.length === 1);
}

// --- состояния и обрыв связи -------------------------------------------------
{
  const page = load("http:");
  page.send({ line: 0, state: "failed" });
  check("сбой провайдера виден", page.statusEl.textContent === "сбой провайдера", page.statusEl.textContent);
  check("сбой выделен цветом", page.statusEl.classList.has("off"));

  page.send({ line: 0, state: "running" });
  check("возврат в работу", page.statusEl.textContent === "на связи", page.statusEl.textContent);

  page.close();
  check("обрыв связи виден", page.statusEl.textContent.startsWith("связь потеряна"), page.statusEl.textContent);
  check("переподключение запланировано", page.timers.length === 1 && page.timers[0].ms === 1500, JSON.stringify(page.timers));
}

// --- размер шрифта по высоте экрана ------------------------------------------
// Заглушка считает высоту текста как «фразы × размер шрифта × 20», поэтому
// проверяются отношения: рост при запасе места, ужимание при нехватке и упор в
// оба предела.
{
  const roomy = load("http:", 900);
  roomy.send({ line: 0, seq: 1, text: "короткая фраза", is_final: true });
  check("на просторном экране шрифт растёт", roomy.lineSize() === 30, String(roomy.lineSize()));

  const tight = load("http:", 300);
  for (let i = 1; i <= 3; i += 1) {
    tight.send({ line: 0, seq: i, text: "фраза " + i, is_final: true });
  }
  check("на тесном экране шрифт подобран по высоте", tight.lineSize() === 5, String(tight.lineSize()));

  const tiny = load("http:", 100);
  for (let i = 1; i <= 3; i += 1) {
    tiny.send({ line: 0, seq: i, text: "фраза " + i, is_final: true });
  }
  check("ужимание упирается в предел", tiny.lineSize() === 2, String(tiny.lineSize()));

  const grown = load("http:", 100);
  grown.send({ line: 0, seq: 1, text: "одна фраза", is_final: true });
  check("одна фраза занимает экран, а не нижний угол", grown.lineSize() === 5, String(grown.lineSize()));
}

console.log(failures === 0 ? "\nВСЕ ПРОВЕРКИ ПРОЙДЕНЫ" : `\nПРОВАЛОВ: ${failures}`);
process.exit(failures === 0 ? 0 : 1);
