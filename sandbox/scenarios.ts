/// 场景表：一条场景 = 把沙盒后端摆成某种状态，界面自己会长成那样。
/// 只改 st 与发事件，绝不直接碰 DOM —— 否则验的就不是真实渲染路径了。
import { cancelUpdate, emit, memLive, pushReadiness, pushStatus, reset, st } from "./sim";

export interface Scenario {
  group: string;
  key: string;
  label: string;
  hint: string;
  run: () => void;
}

const S: Scenario[] = [];

function scenario(group: string, key: string, label: string, hint: string, run: () => void) {
  S.push({ group, key, label, hint, run });
}

function settle(patch: Partial<typeof st>, push = true): void {
  Object.assign(st, patch);
  if (push) {
    pushStatus();
    pushReadiness();
  }
}

// ------------------------------------------------------------------ 起停

scenario("起停", "idle.ready", "未运行·可启动", "开关该是「开始游戏」，可点", () => {
  reset();
  settle({});
});

scenario("起停", "idle.blocked", "未运行·不满足条件", "开关变灰，悬停提示写原因", () => {
  reset();
  settle({
    can_start: false,
    client_ready: false,
    readiness_detail:
      "还没满足启动条件：没找到 DFO.exe，请在设置页选一次客户端目录（D:\\115us\\客户端）",
  });
});

scenario("起停", "idle.gatewaybusy", "未运行·网关被占", "顶栏网关芯片变红", () => {
  reset();
  settle({ gateway_listening: true, can_start: false, readiness_detail: "网关端口已被占用" });
});

scenario("起停", "start.slow70", "启动中·70 秒", "实机那 ~70 秒：开关应是「取消并停止」且可点", () => {
  reset();
  settle({ start_delay_ms: 70000, phase: "starting", detail: "正在拉起服务端与客户端" });
});

scenario("起停", "start.fast", "启动只花 2.5 秒", "改完延迟，再去点开关看转场", () => {
  reset();
  settle({ start_delay_ms: 2500, stop_delay_ms: 800 });
});

scenario("起停", "running", "运行中", "开关应是「停止游戏」", () => {
  reset();
  settle({ phase: "running", gateway_listening: true, detail: "网关已监听 7001" });
});

scenario("起停", "stopping", "收尾中", "开关禁用并显示「正在收尾…」", () => {
  reset();
  settle({ phase: "stopping", gateway_listening: true, detail: "正在收尾" });
});

scenario("起停", "error", "异常", "顶栏芯片转红，开关回到可启动", () => {
  reset();
  settle({ phase: "error", detail: "网关起来了但 60 秒内没等到 7001 监听" });
});

// ------------------------------------------------------------------ 更新

scenario("更新", "upd.none", "没有更新", "点检查更新应提示已是最新", () => {
  reset();
  settle({});
});

scenario("更新", "upd.self", "只有启动器更新", "启动后小红点亮起，点检查后开关变「更新版本」", () => {
  reset();
  settle({ self_latest: "20261017-2126-3bfe4956", asset_mb: 2.7 });
});

scenario("更新", "upd.server", "只有服务端更新", "同上，但走服务端那条链", () => {
  reset();
  settle({ server_latest: "v20261017-1", asset_mb: 96 });
});

scenario("更新", "upd.both", "两条都有", "必须先给启动器：自更完会重启进程", () => {
  reset();
  settle({ self_latest: "20261017-2126-3bfe4956", server_latest: "v20261017-1", asset_mb: 96 });
});

scenario("更新", "upd.pump", "手动逐条发进度", "后端不自己走：只有 __sim.pump(kind, 百分比) 能让环动", () => {
  settle({ update_mode: "manual", update_duration_ms: 6000 });
});

scenario("更新", "upd.stream", "下载·6 秒走完", "进度环从 0 走到 100 再转绿", () => {
  settle({ update_mode: "stream", update_duration_ms: 6000 });
});

scenario("更新", "upd.slow", "下载·40 秒", "看环会不会糊、文案会不会溢出", () => {
  settle({ update_mode: "stream", update_duration_ms: 40000, asset_mb: 1024 });
});

scenario("更新", "upd.instant", "下载·瞬间完成", "只有部署那一段可见", () => {
  settle({ update_mode: "instant", update_duration_ms: 3000 });
});

scenario("更新", "upd.failmid", "下载到 45% 失败", "环必须消失、按钮退回「更新版本」、红条报原因", () => {
  settle({ update_mode: "fail-mid", update_duration_ms: 5000, update_fail_at: 45 });
});

scenario("更新", "upd.stall", "下载到 62% 卡死", "界面应一直锁在「更新中 62%」，直到点「解除卡死」", () => {
  settle({ update_mode: "stall", update_duration_ms: 8000, update_stall_at: 62 });
});

scenario("更新", "upd.pumpfail", "手动判下载失败", "配合 __sim.pumpFail(text)：环消失、退回「更新版本」、红条报原因", () => {
  settle({ update_mode: "manual" });
});

scenario("更新", "upd.unstall", "解除卡死", "让那条永不落地的承诺抛出", () => {
  cancelUpdate();
});

scenario("更新", "upd.portbusy", "网关在听·拒绝部署", "服务端那条必须一句话挡回，不动一个字节", () => {
  settle({ update_mode: "port-busy", server_latest: "v20261017-1", asset_mb: 96 });
});

scenario("更新", "upd.neterr", "检查就失败", "红条：检查更新失败：…", () => {
  settle({ fail_all: true });
});

scenario("更新", "upd.ok", "恢复正常后端", "撤掉整条挂掉与慢响应", () => {
  settle({ fail_all: false, command_latency_ms: 0 });
});

// ------------------------------------------------------------------ 背景

scenario("背景", "bg.image", "壁纸图片", "映射到 fixtures 里的图，应铺满且无白纱（暗色）", () => {
  settle({
    background: { path: "C:\\sandbox\\wallpaper.png", kind: "image", exists: true, note: "", accent: "" },
  });
});

// 亮色 + 液态玻璃只能在高调壁纸上验：暗色 fixtures 里白纱压不压都看不出来，2026-10-18 那次
// 「亮色下糊成一片灰奶」就是拿暗壁纸放过去的。这张是白天高调底，带几条硬边好比对折射带。
scenario("背景", "bg.bright", "高调壁纸（验亮色玻璃）", "亮色主题下卡片应仍是白纱而非灰奶，正文对比不掉", () => {
  settle({
    background: { path: "C:\\sandbox\\wallpaper-bright.png", kind: "image", exists: true, note: "", accent: "" },
  });
});

scenario("背景", "bg.accent", "壁纸 + 主题色", "后端取到 #12a4b9：边框跟色，亮色下整扇窗口一起偏青", () => {
  settle({
    background: {
      path: "C:\\sandbox\\wallpaper.png",
      kind: "image",
      exists: true,
      note: "",
      accent: "#12a4b9",
    },
  });
});

scenario("背景", "bg.video", "动态壁纸（可播）", "有真 mp4 时才成立；视频取不到色，退回默认紫", () => {
  settle({
    background: { path: "C:\\sandbox\\wallpaper.mp4", kind: "video", exists: true, note: "", accent: "" },
  });
});

scenario("背景", "bg.broken", "素材加载失败", "应弹红条，带路径与探测结果", () => {
  settle({
    background: { path: "D:\\没有这个文件.png", kind: "image", exists: true, note: "", accent: "" },
  });
});

scenario("背景", "bg.missing", "素材被搬走", "exists=false：退回纯色，不报错", () => {
  settle({
    background: {
      path: "C:\\Users\\Player\\Pictures\\gone.mp4",
      kind: "video",
      exists: false,
      note: "素材文件不在了",
      accent: "",
    },
  });
});

scenario("背景", "bg.none", "没设置背景", "退回纯色主题", () => {
  settle({
    background: { path: "", kind: "none", exists: false, note: "没设置背景素材", accent: "" },
  });
});

// ------------------------------------------------------------------ 最坏数据

const LONG_PATH =
  "D:\\115us\\服务端\\runtime\\storage\\backups\\2026-10-17-204531-启动前自动备份-这是一段很长很长用来撑破布局的说明文字";

scenario("最坏", "worst.long", "超长路径与账号", "32 字符账号、200 字符路径不许破版", () => {
  settle({
    account: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    accounts: ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "probe", "一个中文账号名试试看"],
    client_dir: LONG_PATH,
    root: LONG_PATH,
    storage_detail: "sqlite：" + LONG_PATH + "\\local.sqlite3（WAL 双文件）",
    readiness_detail: "可以开始：" + LONG_PATH,
  });
});

scenario("最坏", "worst.empty", "全空", "空账号、空列表、空路径不许崩", () => {
  settle({ account: "", accounts: [], client_dir: "", root: "", storage_detail: "" });
});

scenario("最坏", "worst.slow", "每条命令 3 秒", "看有没有重复触发与卡死的按钮", () => {
  settle({ command_latency_ms: 3000 });
});

scenario("最坏", "worst.dead", "后端整条挂掉", "除轮询外全部命令报错", () => {
  settle({ fail_all: true });
});

scenario("最坏", "worst.normal", "恢复默认数据", "把后端状态整个复位", () => {
  reset();
  settle({});
});

// ------------------------------------------------------------------ 事件推送

scenario("事件", "ev.alert", "游戏异常退出提示", "红条：game-alert", () => {
  emit("game-alert", { text: "DFO.exe 以退出码 0xc0000005 结束" });
});

scenario("事件", "ev.portblocked", "端口被占·弹面板", "开始失败时那条端口报告", () => {
  emit("game-port-blocked", {
    where: "启动服务端",
    report: {
      gateway: {
        port: 7001,
        port_label: "7001 网关",
        listening: true,
        pid: 4242,
        image: "dfolauncher.exe",
        kind: "foreign",
        kind_label: "非本启动器进程",
      },
      leftovers: [],
      blocked: [],
      killable: [4242],
      protected_ones: [],
      killed: [],
      failed: [],
      rechecked: true,
      summary: "网关端口被别的实例占着",
    },
    hint: "先结束那个实例，或按「强制清理」",
  });
});

scenario("事件", "ev.firstrun", "首次部署", "没有服务端时那条链", () => {
  emit("first-run-no-server", null);
});

scenario("事件", "ev.serverpush", "后台探到服务端更新", "只点亮小红点，不许弹窗", () => {
  emit("server-update-available", {
    current_tag: "v20261014-1",
    latest_tag: "v20261017-9",
    has_update: true,
    asset_name: "115us-server.zip",
    asset_size: 96 * 1048576,
    download_url: "https://example.invalid/server.zip",
  });
});

scenario("事件", "ev.close", "点标题栏关闭", "应弹三选一，而不是直接走", () => {
  emit("close-requested", {});
});

// ------------------------------------------------------------------ 服务端内存

scenario("内存", "mem.none", "三键都没设、也没采样", "卡上两格空、实况块只有一句说明", () => {
  reset();
  settle({});
});

scenario("内存", "mem.live", "已设限、采样在跑", "尺 + 小曲线 + 六个数", () => {
  reset();
  settle({});
  st.mem_tuning = { server_memory_limit_mib: 4096, go_mem_target_mib: 2048, import_max_procs: 8 };
  st.mem_live = memLive();
});

scenario("内存", "mem.unlimited", "没设硬上限但照样打点", "不画那把尺，硬上限显示「不限」", () => {
  reset();
  settle({});
  st.mem_live = memLive({ hard_limit_mib: null });
});

scenario("内存", "mem.ended", "本场收尾", "峰值出现、芯片改灰，数字仍留着", () => {
  reset();
  settle({});
  st.mem_live = memLive({ ended: true, peak_private_mib: 2099, seconds: 5123.9, private_mib: 0, working_mib: 0 });
});

scenario("内存", "mem.old", "旧编排器 + 采样停了", "两条提醒都要显示，不许摆假数", () => {
  reset();
  settle({});
  st.mem_live = memLive({
    policy: "local-stability-20261008-r1",
    hard_limit_mib: 4096,
    stale: true,
    note: "这一场跑的是旧编排器（policy=local-stability-20261008-r1），本场上限口径按旧版算。",
  });
});

// ------------------------------------------------------------------ 页面与主题

function click(selector: string): void {
  const node = document.querySelector<HTMLElement>(selector);
  if (!node) throw `沙盒：找不到 ${selector}`;
  node.click();
}

scenario("页面", "page.home", "去主页", "点侧边栏主页", () => click('.rail-item[aria-label="主页"]'));
scenario("页面", "page.mods", "去 mod 页", "点侧边栏 mod", () => click('.rail-item[aria-label="mod"]'));
scenario("页面", "page.tools", "去工具页", "点侧边栏工具", () => click('.rail-item[aria-label="工具"]'));
scenario("页面", "page.gm", "去 GM工具页", "点侧边栏 GM工具", () => click('.rail-item[aria-label="GM工具"]'));
scenario("页面", "page.saves", "去存档页", "点侧边栏存档", () => click('.rail-item[aria-label="存档"]'));
scenario("页面", "page.settings", "去设置页", "点侧边栏设置", () => click('.rail-item[aria-label="设置"]'));
scenario("页面", "page.feedback", "去反馈页", "点侧边栏反馈", () => click('.rail-item[aria-label="反馈"]'));
scenario("页面", "page.dev", "去开发者页", "点侧边栏开发者", () => click('.rail-item[aria-label="开发者"]'));
scenario("页面", "page.theme", "切主题", "亮/暗来回切，看背景纱与胶囊", () => {
  click('.win-btn[aria-label^="切换到"]');
});
scenario("页面", "scheme.violet", "配色切到紫罗兰", "在设置页点紫罗兰那块色卡", () => {
  click('.scheme[data-scheme="violet"]');
});
scenario("页面", "scheme.amber", "配色回琥珀", "在设置页点回琥珀那块色卡", () => {
  click('.scheme[data-scheme="amber"]');
});

export const scenarios = S;
