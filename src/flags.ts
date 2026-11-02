/**
 * 功能开关：整合包 V1.0 冻结期把两条更新链与主页更新日志卡暂时摘下来。
 *
 * 摘的是界面入口与启动自检探针，命令层（self_update_check / server_update_check /
 * server_update_apply）原样保留，恢复时把这一格改回 true 即可，不用再翻三处 JSX。
 * 注意：恢复后本地构建仍会显示「有更新」小红点，别点「更新版本」——那条分支会覆盖
 * D:\115us\服务端，把换上的 exe 连同实机确认过的修复一起冲掉（CHANGELOG 2026-10-22 有案）。
 */
export const UPDATES_ENABLED = false;
