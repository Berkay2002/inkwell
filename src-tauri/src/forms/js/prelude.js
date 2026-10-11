// The Acrobat object model that form scripts see, on top of the host functions in `__h` (see engine.rs).
// Adapted from pdfcraft (MIT OR Apache-2.0), crates/js/src/lib.rs @ b8ddf71, whose object model was written
// against boa's native-function API; here it is plain JavaScript over a few string-in, string-out host calls.
// Loaded once into every runtime, before the document's own scripts. Nothing here reaches a file or the network.
(function (g) {
  'use strict';
  const H = g.__h;
  delete g.__h;
  // Test builds only: a host call that cannot be interrupted.
  if (H.sleep) g.__sleep = H.sleep;
  const Str = String;
  const J = JSON.stringify.bind(JSON);
  const P = JSON.parse.bind(JSON);
  const defProp = Object.defineProperty.bind(Object);
  const CAP = 65536;
  const cap = (s) => (s.length > CAP ? s.slice(0, CAP) : s);
  // Text handed to the host: undefined and null are empty, everything is cut at 64 KiB.
  const S = (v) => (v === undefined || v === null ? '' : cap(Str(v)));
  const hide = (o, k, v) => defProp(o, k, { value: v, writable: true, configurable: true, enumerable: false });
  const toNum = (v) => {
    const n = Number(v);
    return n === n ? n : 0;
  };

  // ---- calls this host does not implement: they do nothing, and the script log says so ---------------
  const stubs = new Map();
  const unsupported = (label) => {
    let f = stubs.get(label);
    if (!f) {
      f = function () {
        H.unsupported(label);
        return undefined;
      };
      stubs.set(label, f);
    }
    return f;
  };
  // An object whose unknown (string-named, not inherited) properties are such do-nothing functions.
  const softly = (target, prefix) =>
    new Proxy(target, {
      get(t, p, r) {
        if (typeof p !== 'string' || p in t) return Reflect.get(t, p, r);
        return unsupported(prefix + p);
      },
    });

  // ---- fields ----------------------------------------------------------------------------------------
  const FIELD_STUBS = [
    'setAction', 'setItems', 'insertItemAt', 'deleteItemAt', 'clearItems', 'signatureSign', 'signatureValidate',
    'signatureInfo', 'signatureSetSeedValue', 'signatureGetSeedValue', 'signatureGetModifications', 'browseForFileToSubmit',
    'buttonGetCaption', 'buttonSetCaption', 'buttonGetIcon', 'buttonSetIcon', 'buttonImportIcon', 'getArray', 'getLock',
    'setLock', 'defaultIsChecked', 'checkThisBoxDefault', 'isDefaultChecked',
  ];
  const cache = new Map();
  const get = (name, prop) => P(H.fget(name, prop));
  const set = (name, prop, v) => H.fset(name, prop, J(v === undefined ? null : v));

  function makeField(name) {
    const f = {};
    const acc = (k, getter, setter) => defProp(f, k, { get: getter, set: setter, enumerable: true, configurable: true });
    const ro = (k, prop) => acc(k, () => get(name, prop));
    const rw = (k, prop) => acc(k, () => get(name, prop), (v) => set(name, prop, v));
    rw('value', 'value');
    ro('valueAsString', 'valueAsString');
    ro('defaultValue', 'defaultValue');
    ro('type', 'type');
    ro('name', 'name');
    rw('readonly', 'readonly');
    rw('required', 'required');
    rw('display', 'display');
    rw('hidden', 'hidden');
    ro('numItems', 'numItems');
    ro('charLimit', 'charLimit');
    ro('exportValues', 'exportValues');
    ro('currentValueIndices', 'currentValueIndices');
    rw('textColor', 'textColor');
    rw('fillColor', 'fillColor');
    rw('strokeColor', 'strokeColor');
    hide(f, 'getItemAt', (i, exp) => get(name, 'item:' + toNum(i) + ':' + (exp === undefined || exp ? 1 : 0)));
    hide(f, 'isBoxChecked', (i) => get(name, 'checked:' + toNum(i)));
    hide(f, 'checkThisBox', (i, on) => set(name, 'check', [toNum(i), on === undefined ? true : !!on]));
    hide(f, 'setFocus', () => H.req('focus', name));
    for (const m of FIELD_STUBS) hide(f, m, unsupported('field.' + m));
    return f;
  }

  function getField(name) {
    const canon = H.fexists(S(name));
    if (!canon) return null;
    let f = cache.get(canon);
    if (!f) {
      f = makeField(canon);
      cache.set(canon, f);
    }
    return f;
  }

  // ---- the document (this, and the global object) -----------------------------------------------------
  hide(g, 'getField', getField);
  hide(g, 'getNthFieldName', (i) => H.nth(toNum(i)) || null);
  defProp(g, 'numFields', { get: () => H.nfields(), configurable: true });
  const docInfo = () => P(H.doc_info());
  defProp(g, 'numPages', { get: () => docInfo().numPages, configurable: true });
  defProp(g, 'documentFileName', { get: () => docInfo().fileName, configurable: true });
  defProp(g, 'info', { get: () => docInfo().info, configurable: true });
  defProp(g, 'pageNum', {
    get: () => docInfo().page,
    set: (n) => H.req('page', Str(toNum(n))),
    configurable: true,
  });
  defProp(g, 'dirty', { get: () => false, set: () => {}, configurable: true });
  hide(g, 'resetForm', (names) => H.req('reset', J(names === undefined || names === null ? [] : Array.isArray(names) ? names.map(S) : [S(names)])));
  hide(g, 'print', () => H.req('print', ''));
  hide(g, 'submitForm', (a) => H.req('submit', typeof a === 'object' && a !== null && !Array.isArray(a) ? S(a.cURL) : S(a)));
  hide(g, 'getURL', (u) => H.req('launch', S(u)));
  hide(g, 'calculateNow', () => H.req('calculate', ''));
  hide(g, 'syncAnnotScan', () => {});
  for (const m of [
    'addAnnot', 'addField', 'addIcon', 'addLink', 'addScript', 'addWatermarkFromText', 'appendPage', 'bringToFront', 'closeDoc',
    'createDataObject', 'deletePages', 'exportAsFDF', 'exportAsXFDF', 'exportDataObject', 'extractPages', 'flattenPages',
    'getAnnot', 'getAnnots', 'getDataObject', 'getLinks', 'getOCGs', 'getPageBox', 'getPageLabel', 'getPageNthWord',
    'getPageNumWords', 'getPageRotation', 'getPrintParams', 'gotoNamedDest', 'importAnFDF', 'importAnXFDF', 'importDataObject',
    'importIcon', 'importTextData', 'insertPages', 'mailDoc', 'mailForm', 'movePage', 'newPage', 'openDataObject',
    'removeDataObject', 'removeField', 'removeLinks', 'removeScript', 'replacePages', 'saveAs', 'setAction', 'setPageAction',
    'setPageLabels', 'setPageRotations', 'spawnPageFromTemplate', 'AFParseDateEx', 'AFExactMatch',
  ]) {
    hide(g, m, unsupported(m));
  }
  hide(g, 'global', {});

  // ---- app ---------------------------------------------------------------------------------------------
  // The answer to a question the script asks (Yes/No, OK/Cancel) is the safe one: No, Cancel.
  const answer = (type) => (type === 2 || type === 3 ? 3 : type === 1 ? 2 : 1);
  const app = softly(
    {
      alert(msg, icon, type, title) {
        let text = msg;
        let t = type;
        let ttl = title;
        if (typeof msg === 'object' && msg !== null) {
          text = msg.cMsg;
          t = msg.nType;
          ttl = msg.cTitle;
        }
        H.alert(S(text), S(ttl));
        return answer(toNum(t));
      },
      beep: () => H.req('beep', ''),
      response: () => null,
      launchURL(u) {
        H.req('launch', typeof u === 'object' && u !== null ? S(u.cURL) : S(u));
      },
      setTimeOut() {
        H.unsupported('app.setTimeOut (timers never run)');
        return {};
      },
      setInterval() {
        H.unsupported('app.setInterval (timers never run)');
        return {};
      },
      clearTimeOut() {},
      clearInterval() {},
      viewerType: 'Reader',
      viewerVariation: 'Reader',
      viewerVersion: 24.0,
      formsVersion: 24.0,
      platform: H.platform(),
      language: 'ENU',
    },
    'app.'
  );
  hide(g, 'app', app);

  // ---- util --------------------------------------------------------------------------------------------
  const util = softly(
    {
      printf(fmt, ...args) {
        const r = H.printf(S(fmt), J(args.map((a) => [Number(a), S(a)])));
        if (r.startsWith('\u0000ERR:')) throw new RangeError(r.slice(5));
        return r;
      },
      printd(fmt, d) {
        if (!(d instanceof Date) || isNaN(d.getTime())) throw new TypeError('util.printd needs a Date');
        const parts = [d.getFullYear(), d.getMonth(), d.getDate(), d.getDay(), d.getHours(), d.getMinutes(), d.getSeconds()];
        return typeof fmt === 'number' ? H.printd('', fmt, J(parts)) : H.printd(S(fmt), -1, J(parts));
      },
      printx: (fmt, src) => H.printx(S(fmt), S(src)),
      scand(fmt, text) {
        const r = P(H.scand(S(fmt), S(text)));
        return r ? new Date(r[0], r[1] - 1, r[2], r[3], r[4], r[5]) : null;
      },
    },
    'util.'
  );
  hide(g, 'util', util);

  const consoleObj = softly(
    {
      println: (m) => H.log('info', S(m)),
      show() {},
      hide() {},
      clear() {},
    },
    'console.'
  );
  hide(g, 'console', consoleObj);

  hide(g, 'display', { visible: 0, hidden: 1, noPrint: 2, noView: 3 });
  hide(g, 'color', {
    transparent: ['T'],
    black: ['G', 0],
    white: ['G', 1],
    red: ['RGB', 1, 0, 0],
    green: ['RGB', 0, 1, 0],
    blue: ['RGB', 0, 0, 1],
    cyan: ['CMYK', 1, 0, 0, 0],
    magenta: ['CMYK', 0, 1, 0, 0],
    yellow: ['CMYK', 0, 0, 1, 0],
    dkGray: ['G', 0.25],
    gray: ['G', 0.5],
    ltGray: ['G', 0.75],
    equal: (a, b) => J(a) === J(b),
    convert: unsupported('color.convert'),
  });

  // ---- Acrobat's AF functions, over the same Rust code that runs the fast path ------------------------
  const args = (a) => J(Array.prototype.slice.call(a));
  const fmt = (call) =>
    function () {
      event.value = H.af_format(call, args(arguments), S(event.value));
    };
  const key = (call) =>
    function () {
      const r = P(
        H.af_key(call, args(arguments), S(event.targetName), S(event.value), JSON.stringify({ change: S(event.change), commit: !!event.willCommit, start: toNum(event.selStart), end: toNum(event.selEnd) }))
      );
      if (!r.ok) {
        event.rc = false;
        if (r.msg) H.alert(r.msg, '');
      } else if (r.value !== undefined) {
        event.value = r.value;
      }
    };
  for (const [name, call] of [
    ['AFNumber', 'AFNumber_Format'],
    ['AFPercent', 'AFPercent_Format'],
    ['AFDate', 'AFDate_Format'],
    ['AFTime', 'AFTime_Format'],
    ['AFSpecial', 'AFSpecial_Format'],
  ]) {
    hide(g, name + '_Format', fmt(call));
    hide(g, name + '_Keystroke', key(call));
  }
  hide(g, 'AFDate_FormatEx', fmt('AFDate_FormatEx'));
  hide(g, 'AFDate_KeystrokeEx', key('AFDate_FormatEx'));
  hide(g, 'AFTime_FormatEx', fmt('AFTime_FormatEx'));
  hide(g, 'AFTime_KeystrokeEx', key('AFTime_FormatEx'));
  hide(g, 'AFSpecial_KeystrokeEx', key('AFSpecial_KeystrokeEx'));
  hide(g, 'AFRange_Validate', function () {
    const r = P(H.af_range(args(arguments), S(event.value)));
    if (!r.ok) {
      event.rc = false;
      H.alert(r.msg, '');
    }
  });
  hide(g, 'AFSimple_Calculate', function (op, fields) {
    const list = Array.isArray(fields) ? fields.map(S) : S(fields).split(',').map((s) => s.trim()).filter(Boolean);
    const r = H.af_calc(S(op), J(list));
    event.value = r === '' ? 0 : Number(r);
  });
  hide(g, 'AFMakeNumber', (v) => (typeof v === 'number' ? v : P(H.af_number(S(v)))));
  hide(g, 'AFMergeChange', (e) => {
    if (e.willCommit) return S(e.value);
    const v = S(e.value);
    const a = Math.min(Math.max(toNum(e.selStart), 0), v.length);
    const b = Math.min(Math.max(toNum(e.selEnd), a), v.length);
    return v.slice(0, a) + S(e.change) + v.slice(b);
  });

  // ---- events --------------------------------------------------------------------------------------------
  hide(g, '__begin', (json) => {
    const e = P(json);
    e.rc = true;
    e.target = e.targetName ? getField(e.targetName) : null;
    e.changeEx = e.change;
    g.event = e;
  });
  hide(g, '__end', () => {
    const e = g.event;
    let v = e.value;
    if (typeof v === 'number') v = isFinite(v) ? Str(v) : '';
    return J({ rc: e.rc === undefined ? true : !!e.rc, value: S(v), change: S(e.change) });
  });
})(globalThis);
