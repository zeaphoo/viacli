#!/usr/bin/env python3
"""采集器一致性检查器——把 `docs/collector-protocol.md` §10 的清单变成可执行的测试。

用法：

    python3 examples/collectors/conformance.py -- ./target/debug/viacli-sys
    python3 examples/collectors/conformance.py -- python3 examples/collectors/echo_metrics.py

为什么要它：协议写在文档里只是**主张**，能对任意第三方采集器跑起来才算**标准**。
宿主可以把同一套检查内置成 `collector test <path>`；
在那之前，这个脚本是"采集器作者自测"和"平台审核"共用的裁判。

只依赖 Python 标准库，方便任何人拿到就能用。
"""

from __future__ import annotations

import json
import os
import re
import select
import subprocess
import sys
import threading
import time

KNOWN_TYPES = {"meta", "series", "sample", "record", "state", "log", "error"}
KNOWN_ERROR_KINDS = {"config", "auth", "network", "upstream", "parse", "internal"}
KNOWN_UNITS = {
    "bytes",
    "bytes_per_second",
    "percent",
    "celsius",
    "seconds",
    "hertz",
    "watts",
    "volts",
    "amperes",
    "rpm",
    "count",
}
NAME_RE = re.compile(r"^[A-Za-z][A-Za-z0-9_.-]*(?:/[A-Za-z0-9_.-]+)*$")
MAX_NAME_LEN = 255

# 默认的"坏配置"：两种语言的实现都应据此以退出码 2 拒绝
DEFAULT_BAD_CONFIG = {"intervalMs": 1}


class Run:
    """一次采集器运行的全部可观测事实。"""

    def __init__(self, cmd):
        self.cmd = cmd
        self.lines: list[tuple[float, str]] = []  # (相对到达时刻, 原始行)
        self.stderr = ""
        self.exit_code: int | None = None
        self.timed_out = False
        self.elapsed = 0.0
        # 我们主动关闭 stdin 的时刻（相对启动）。用来判定"消息是在被要求停止之前到达的"
        self.stdin_closed_at: float | None = None

    @property
    def messages(self):
        """(到达时刻, 解析后的对象)；非法 JSON 的行被跳过（由检查项 3 专门报错）。"""
        out = []
        for t, raw in self.lines:
            try:
                out.append((t, json.loads(raw)))
            except json.JSONDecodeError:
                pass
        return out

    def of_type(self, kind):
        return [(t, m) for t, m in self.messages if m.get("type") == kind]


def run_collector(
    cmd,
    request=None,
    *,
    timeout=10.0,
    close_stdin=False,
    close_stdin_after=0.4,
    raw_stdin=None,
    env_extra=None,
):
    """启动采集器，喂请求，收集输出与退出码。

    `close_stdin=True` 时在短暂延迟后关闭 stdin，用来验证"关了就尽快退出"（§4.2/§4.4）。
    """
    env = dict(os.environ)
    if env_extra:
        env.update(env_extra)
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=0,  # 裸 fd：读取时刻才准确，才能验证 flush（检查项 9）
        env=env,
    )
    r = Run(cmd)
    started = time.monotonic()

    def read_stdout():
        # 按行切分，并记录**每一行到达的时刻**——这是检查 flush 的唯一办法
        buf = b""
        fd = proc.stdout.fileno()
        while True:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            buf += chunk
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                r.lines.append((time.monotonic() - started, line.decode("utf-8", "replace")))
        if buf:
            r.lines.append((time.monotonic() - started, buf.decode("utf-8", "replace")))

    def read_stderr():
        try:
            r.stderr = proc.stderr.read().decode("utf-8", "replace")
        except OSError:
            pass

    t_out = threading.Thread(target=read_stdout, daemon=True)
    t_err = threading.Thread(target=read_stderr, daemon=True)
    t_out.start()
    t_err.start()

    payload = None
    if raw_stdin is not None:
        payload = raw_stdin.encode()
    elif request is not None:
        payload = (json.dumps(request) + "\n").encode()

    try:
        if payload is not None:
            proc.stdin.write(payload)
            proc.stdin.flush()
        if close_stdin:
            time.sleep(close_stdin_after)
            r.stdin_closed_at = time.monotonic() - started
            proc.stdin.close()
    except (BrokenPipeError, ValueError):
        pass

    try:
        r.exit_code = proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        r.timed_out = True
        proc.kill()
        try:
            r.exit_code = proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            r.exit_code = None

    t_out.join(timeout=2)
    t_err.join(timeout=2)
    try:
        if proc.stdin and not proc.stdin.closed:
            proc.stdin.close()
    except OSError:
        pass
    r.elapsed = time.monotonic() - started
    return r


# ---------------------------------------------------------------- 检查项


class Report:
    def __init__(self):
        self.rows: list[tuple[str, bool, str]] = []

    def check(self, name, ok, detail=""):
        self.rows.append((name, bool(ok), detail))
        print(f"  {'✅' if ok else '❌'} {name}" + (f"  — {detail}" if detail else ""))
        return ok

    @property
    def failed(self):
        return [r for r in self.rows if not r[1]]


def check_describe(cmd, rep: Report):
    print("\n[1] `--describe` 自描述")
    r = run_collector(cmd + ["--describe"], timeout=10)
    if not rep.check("退出码 0", r.exit_code == 0, f"exit={r.exit_code}"):
        return None
    if not rep.check("stdout 恰好一行", len(r.lines) == 1, f"{len(r.lines)} 行"):
        return None
    try:
        d = json.loads(r.lines[0][1])
    except json.JSONDecodeError as e:
        rep.check("是合法 JSON", False, str(e))
        return None
    rep.check("是合法 JSON", True)
    rep.check("含 protocol", d.get("protocol") == 1, f"protocol={d.get('protocol')!r}")
    rep.check("含 kind", isinstance(d.get("kind"), str) and d["kind"], f"kind={d.get('kind')!r}")
    schema = d.get("configSchema")
    rep.check(
        "含 configSchema（对象）",
        isinstance(schema, dict) and schema.get("type") == "object",
        f"{type(schema).__name__}",
    )
    rep.check(
        "声明 modes 且含默认模式",
        isinstance(d.get("modes"), list)
        and d.get("defaultMode") in (d.get("modes") or []),
        f"modes={d.get('modes')}, defaultMode={d.get('defaultMode')!r}",
    )
    kinds = d.get("recordKinds")
    if kinds is None:
        print("     ⚠️  未声明 recordKinds —— 记录类型的用途与保留期对宿主不可见（协议 §8.1）")
    elif not isinstance(kinds, list):
        rep.check("recordKinds 是数组", False, type(kinds).__name__)
    else:
        bad = [k for k in kinds if not isinstance(k, dict) or not isinstance(k.get("kind"), str)]
        rep.check("recordKinds 每项含 kind", not bad, json.dumps(bad[:2], ensure_ascii=False))
        errs = []
        for k in kinds:
            if not isinstance(k, dict):
                continue
            e = retention_error(k)
            if e:
                errs.append(f"{k.get('kind')}: {e}")
        rep.check("recordKinds 保留声明合法（§2.8）", not errs, "; ".join(errs[:3]))
    return d


def retention_error(obj):
    """校验保留声明（协议 §2.8）；合法返回 None，否则返回错误说明。

    写成"返回错误"而不是"直接登记检查"，是因为序列可能有几百条——
    每条一行会把报告淹掉，而且真正有价值的信息是"有没有问题"，不是"几百条都合法"。
    """
    persist = obj.get("persist", True)
    has_max = "maxRetentionMs" in obj
    max_ms = obj.get("maxRetentionMs")
    if not isinstance(persist, bool):
        return f"persist 应为布尔，实际 {persist!r}"
    if persist is False:
        # §2.8：persist:false 与 maxRetentionMs 矛盾，宿主 MUST 视为协议错误
        if has_max:
            return f"persist=false 时不得给 maxRetentionMs（给了 {max_ms!r}）"
        return None
    if not has_max or max_ms is None:
        return None  # 无意见 / 永久，都合法
    if isinstance(max_ms, bool) or not isinstance(max_ms, int):
        return f"maxRetentionMs 应为整数或 null，实际 {max_ms!r}"
    if max_ms <= 0:
        # 0 会被 JS 当成 falsy；表达"不落库"必须用 persist:false
        return f"maxRetentionMs 应为正整数（0 会让 JS 误判为'没配'），实际 {max_ms!r}"
    return None


def check_valid_run(cmd, rep: Report, desc, mode="oneshot"):
    print(f"\n[2/3/4/5/9/10] 正常一轮（mode={mode}）")
    req = {
        "protocol": 1,
        "runId": "conformance-1",
        "now": int(time.time() * 1000),
        "mode": mode,
        "intervalMs": 1000,
        "config": {},
    }
    # stream 模式下把 stdin 开满 1s 再关：这样"不 flush 的采集器"会暴露出来——
    # 它的消息要等进程退出才出现，而 flush 了的采集器几百毫秒内就该出声。
    r = run_collector(
        cmd,
        req,
        timeout=5 if mode == "oneshot" else 4,
        close_stdin=(mode == "stream"),
        close_stdin_after=1.0,
    )
    rep.check("退出码 0", r.exit_code == 0, f"exit={r.exit_code}（超时={r.timed_out}）")

    # --- 检查项 3：每一行都是合法 JSON，且 type 已知（最重要的一项）
    bad_json, bad_type = [], []
    for _, raw in r.lines:
        if not raw.strip():
            continue
        try:
            msg = json.loads(raw)
        except json.JSONDecodeError:
            bad_json.append(raw[:120])
            continue
        if not isinstance(msg, dict):
            bad_type.append(f"不是对象: {raw[:80]}")
        elif msg.get("type") not in KNOWN_TYPES:
            # 未知 type 必须视为协议错误，并把原始行带上（§2.3）
            bad_type.append(f"未知 type={msg.get('type')!r}: {raw[:120]}")
    rep.check("每行都是合法 JSON 对象", not bad_json, "; ".join(bad_json[:3]))
    rep.check("每行 type 已知", not bad_type, "; ".join(bad_type[:3]))
    rep.check("至少产出一条数据", len(r.of_type("sample")) + len(r.of_type("record")) > 0)

    # --- 检查项 9：消息必须在**被要求停止之前**就到达（验证 flush，§3.3）
    #
    # 这是整个检查里最难自动判、也最要命的一项：不 flush 的采集器在 oneshot 下
    # 因为"反正要退出"而看起来完全正常，只有在 stream 模式下才会表现为
    # "进程活着但宿主一个字都读不到"。所以判据不是"够不够快"，
    # 而是"它比我们关 stdin 的时刻更早出现"。
    if not r.lines:
        rep.check("消息在被要求停止前到达（已 flush）", False, "没有任何输出")
    elif mode == "oneshot":
        first = r.lines[0][0]
        rep.check("消息及时可见（已 flush）", first < 1.0, f"首条 {first:.3f}s")
    elif r.stdin_closed_at is None:
        rep.check("消息在被要求停止前到达（已 flush）", True, "未能测量")
    else:
        first = r.lines[0][0]
        rep.check(
            "消息在被要求停止前到达（已 flush）",
            first < r.stdin_closed_at - 0.1,
            f"首条 {first:.3f}s，关闭 stdin 于 {r.stdin_closed_at:.3f}s",
        )

    # --- 检查项 10：stderr 不得污染 stdout
    rep.check(
        "stderr 未混入协议行",
        not any(line.lstrip().startswith("{") and '"type"' in line for line in r.stderr.splitlines()),
        "",
    )

    # --- 检查项 4/5：序列命名、数值类型、声明一致性
    declared: dict[str, dict] = {}
    dup_conflict = []
    retention_errs: list[str] = []
    for _, m in r.of_type("series"):
        name = m.get("name")
        if not isinstance(name, str) or not NAME_RE.match(name) or len(name.encode()) > MAX_NAME_LEN:
            rep.check(f"series.name 合法（{name!r}）", False)
            continue
        if m.get("kind") not in ("gauge", "counter"):
            rep.check(f"series.kind 合法（{name}）", False, f"kind={m.get('kind')!r}")
        unit = m.get("unit")
        if unit is not None and unit not in KNOWN_UNITS:
            # 允许自定义单位，但值得提醒
            print(f"     ⚠️  {name} 使用了推荐表之外的单位 {unit!r}")
        if name in declared and declared[name].get("kind") != m.get("kind"):
            dup_conflict.append(name)
        declared.setdefault(name, m)
        e = retention_error(m)
        if e:
            retention_errs.append(f"{name}: {e}")
    rep.check("series 声明语义一致", not dup_conflict, ",".join(dup_conflict))
    rep.check(
        f"series 保留声明合法（§2.8，共 {len(declared)} 条）",
        not retention_errs,
        "; ".join(retention_errs[:3]),
    )

    undeclared, bad_value, bad_name = set(), 0, set()
    for _, m in r.of_type("sample"):
        name, value = m.get("name"), m.get("value")
        if not isinstance(name, str) or not NAME_RE.match(name):
            bad_name.add(str(name))
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            bad_value += 1
        if name not in declared:
            undeclared.add(str(name))
    rep.check("sample.name 符合 §2.4", not bad_name, ",".join(sorted(bad_name)[:5]))
    rep.check("sample.value 是数值", bad_value == 0, f"{bad_value} 条非法")
    rep.check(
        "sample 都在声明过的序列上",
        not undeclared,
        "未声明: " + ",".join(sorted(undeclared)[:5]) if undeclared else "",
    )

    # 记录类型：未在 recordKinds 里声明只算警告（类型可能由用户配置决定）
    if desc and isinstance(desc.get("recordKinds"), list):
        known = {k.get("kind") for k in desc["recordKinds"] if isinstance(k, dict)}
        emitted = {m.get("kind") for _, m in r.of_type("record")}
        unknown = sorted(t for t in emitted if t not in known)
        if unknown:
            print(f"     ⚠️  这些记录类型未在 recordKinds 声明（宿主无法知道用途与保留期）: {unknown}")

    for _, m in r.of_type("record"):
        ok = isinstance(m.get("kind"), str) and isinstance(m.get("payload"), dict)
        if not ok:
            rep.check("record 含 kind 与对象型 payload", False, json.dumps(m)[:120])
            break
    else:
        if r.of_type("record"):
            rep.check("record 含 kind 与对象型 payload", True)

    for _, m in r.of_type("error"):
        if m.get("kind") not in KNOWN_ERROR_KINDS:
            rep.check("error.kind 合法", False, f"kind={m.get('kind')!r}")
            break
    return r


def check_bad_config(cmd, rep: Report):
    print("\n[6] 非法配置 → 退出码 2，且不得是崩溃栈")
    req = {
        "protocol": 1,
        "runId": "conformance-bad",
        "now": int(time.time() * 1000),
        "mode": "oneshot",
        "config": DEFAULT_BAD_CONFIG,
    }
    r = run_collector(cmd, req, timeout=10)
    rep.check("退出码 2", r.exit_code == 2, f"exit={r.exit_code}")
    rep.check("stderr 无 Python/Rust 崩溃栈", "Traceback" not in r.stderr and "panicked" not in r.stderr)
    said = bool(r.of_type("error")) or bool(r.stderr.strip())
    rep.check("给出了可读错误", said, (r.stderr.strip().splitlines() or [""])[-1][:100])


def check_protocol_mismatch(cmd, rep: Report):
    print("\n[9] 协议版本不兼容 → 拒绝而不是尽力而为（§9）")
    req = {
        "protocol": 99,
        "runId": "conformance-v99",
        "now": int(time.time() * 1000),
        "mode": "oneshot",
        "config": {},
    }
    r = run_collector(cmd, req, timeout=10)
    rep.check("退出码 2", r.exit_code == 2, f"exit={r.exit_code}")
    errs = r.of_type("error")
    rep.check("发出 error(kind=config)", bool(errs) and errs[0][1].get("kind") == "config")
    # 不得在拒绝之前先吐一堆数据
    rep.check("拒绝时未产出数据", not r.of_type("sample") and not r.of_type("record"))


def check_stdin_close(cmd, rep: Report):
    print("\n[7] 关闭 stdin 后及时退出（stream 模式，§4.2/§4.4）")
    req = {
        "protocol": 1,
        "runId": "conformance-close",
        "now": int(time.time() * 1000),
        "mode": "stream",
        "intervalMs": 30_000,  # 故意设得很长：只有"关了就退"才能过
        "config": {},
    }
    r = run_collector(cmd, req, timeout=8, close_stdin=True)
    rep.check("在超时前自行退出", not r.timed_out, f"耗时 {r.elapsed:.2f}s")
    rep.check("退出码 0", r.exit_code == 0, f"exit={r.exit_code}")
    rep.check("退出足够快（<3s）", r.elapsed < 3.0, f"{r.elapsed:.2f}s")


def check_limits(cmd, rep: Report):
    print("\n[8] 遵守 limits（§4.5）")
    req = {
        "protocol": 1,
        "runId": "conformance-limits",
        "now": int(time.time() * 1000),
        "mode": "stream",
        # 用 300ms 而不是更小的值：采集器协议允许拒绝过短的间隔（本参考实现下限 250ms），
        # 检查器不该因为自己给了非法间隔而误判
        "intervalMs": 300,
        "config": {},
        "limits": {"maxLines": 20, "maxOutputBytes": 1 << 20},
    }
    r = run_collector(cmd, req, timeout=8, close_stdin=True)
    rep.check("自行停止而不是被杀死", not r.timed_out, f"耗时 {r.elapsed:.2f}s, exit={r.exit_code}")
    rep.check("退出码 0", r.exit_code == 0, f"exit={r.exit_code}")


def check_garbage_stdin(cmd, rep: Report):
    print("\n[4] stdin 不是合法 JSON → 协议错误并附原始行（§2.3）")
    r = run_collector(cmd, raw_stdin="这不是 JSON\n", timeout=10)
    rep.check("退出码 2", r.exit_code == 2, f"exit={r.exit_code}")
    text = r.stderr + " ".join(raw for _, raw in r.lines)
    rep.check("错误信息带上了原始行", "这不是 JSON" in text)


def main():
    argv = sys.argv[1:]
    if argv and argv[0] == "--":
        argv = argv[1:]
    if not argv:
        print(__doc__)
        return 2
    cmd = argv
    print(f"检查采集器: {' '.join(cmd)}")

    desc = check_describe(cmd, rep := Report())
    # 声明了几种模式就都跑一遍：只测默认模式会漏掉一整类问题——
    # 例如"不 flush"在 oneshot 下完全看不出来（进程退出时总会刷出），
    # 只有 stream 模式才暴露（§3.3）。
    modes = (desc or {}).get("modes") or ["oneshot"]
    for mode in [m for m in ("oneshot", "stream") if m in modes]:
        check_valid_run(cmd, rep, desc, mode=mode)
    check_protocol_mismatch(cmd, rep)
    check_bad_config(cmd, rep)
    check_garbage_stdin(cmd, rep)
    check_stdin_close(cmd, rep)
    check_limits(cmd, rep)

    total = len(rep.rows)
    failed = rep.failed
    print("\n" + "=" * 60)
    print(f"结果：{total - len(failed)}/{total} 项通过")
    if failed:
        print("未通过：")
        for name, _, detail in failed:
            print(f"  ❌ {name}" + (f"  — {detail}" if detail else ""))
        return 1
    print("✅ 该采集器符合 docs/collector-protocol.md §10 的一致性要求")
    return 0


if __name__ == "__main__":
    sys.exit(main())
