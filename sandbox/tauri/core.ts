import { dispatch } from "../sim";

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return (await dispatch(cmd, args)) as T;
}

/// 真机上这条走 asset 协议，白名单没放行就是 403 —— 沙盒里换成 fixtures 映射：
/// 映射不到就返回一个必然加载失败的地址，正好用来验背景层的报错文案。
export function convertFileSrc(filePath: string): string {
  const map = (window as unknown as { __assets?: Record<string, string> }).__assets ?? {};
  return map[filePath] ?? "http://asset.localhost/forbidden/" + encodeURIComponent(filePath);
}
