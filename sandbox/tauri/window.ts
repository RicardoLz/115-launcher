const listeners = new Map<string, Set<(payload: unknown) => void>>();

function fire(name: string, payload: unknown): void {
  for (const handler of listeners.get(name) ?? []) handler(payload);
}

export function __fire(name: string, payload: unknown): void {
  fire(name, payload);
}

function subscribe(name: string, handler: (payload: unknown) => void) {
  let set = listeners.get(name);
  if (!set) {
    set = new Set();
    listeners.set(name, set);
  }
  set.add(handler);
  return () => {
    set?.delete(handler);
  };
}

const state = { maximized: false, focused: true };

const base: Record<string, unknown> = {
  label: "main",
  isMaximized: async () => state.maximized,
  toggleMaximize: async () => {
    state.maximized = !state.maximized;
    fire("tauri://resize", {});
  },
  maximize: async () => {
    state.maximized = true;
  },
  unmaximize: async () => {
    state.maximized = false;
  },
  minimize: async () => {},
  close: async () => {
    fire("close-requested", {});
  },
  startDragging: async () => {},
  setFocus: async () => {},
  show: async () => {},
  hide: async () => {},
  outerSize: async () => ({ width: 1280, height: 720 }),
  innerSize: async () => ({ width: 1280, height: 720 }),
  scaleFactor: async () => 1,
  onCloseRequested: (handler: (event: { preventDefault: () => void }) => void) => {
    const wrapped = () => handler({ preventDefault: () => {} });
    return Promise.resolve(subscribe("close-requested", wrapped));
  },
  onResized: (handler: () => void) => Promise.resolve(subscribe("tauri://resize", handler)),
  onFocusChanged: (handler: (payload: { payload: boolean }) => void) =>
    Promise.resolve(subscribe("tauri://focus", () => handler({ payload: state.focused }))),
};

export function getCurrentWindow() {
  return new Proxy(base, {
    get(target, key: string) {
      if (key in target) return target[key];
      return async () => undefined;
    },
  });
}
