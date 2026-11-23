# 实验：同一份 4.0.4.0 整包走 `adb sideload`（`/data` 关着）

配套 `docs/rebase-4.0.4.0.md` §7.3。**merge 链在这台机上已经证明是通的**（系统 OTA 走通了同样的
快照 + 首启 merge），所以这个实验不再回答「merge 通不通」，只回答一件事：**我们这条路径写出来的
A 槽，不带快照，能不能起。**

## 0. 读到什么算什么

| B 组（sideload）的结果 | 结论 |
| --- | --- |
| 首启进得了系统 | 病因锁死在**快照线**（§7.2 第 1/2 条）：干净覆盖写能起，说明我们写进去的镜像与 AVB 都对，差别只在 COW 与 state |
| 首启照样起不来（起来又倒） | 连不带快照的干净 A 槽都起不来：问题不在快照，而在写进去的东西。核 `vbmeta_a` 与包内那段是否逐字节一致（§7.3 第二行；`reflash`/`installed_boot_set()` 两条已在 §7.2 排除） |
| 根本没装进去（引擎拒 / 我们拒 / 断链） | 两个假设都还没验到。按 §6 记下拒在哪一步再回来 |

## 1. 为什么换来源就能换掉 `/data` 的状态

- **内部存储**：包本身就在 `/sdcard`（`/data`），必须先解密；引擎有 `/data` 就能建 COW → 状态
  `Snapshotted`、`merge outstanding true`，首启必须 merge。
- **电脑（`adb sideload`）**：包经 adb 进来，由 FUSE 现取成 `/sideload/package.zip`，**`/data`
  一步都不用开**。`update_system` 全流程没有对 `/data` 的检查（唯一的 `Refusal::DataClosed`
  在 `run.rs:484` 的 `install_system`，只用于把第二个系统装进 `/data`）；`apply` 也不传任何
  data 目录参数，引擎自己判断（`ota.rs:11-18` 只有 `--payload` / `--offset` / `--headers` /
  `--status_fd`）。
- 没有 `/data`，引擎建不了 COW，只剩覆盖写入这一条：包会在
  `dynamic_partition_control_android` 里打 `Will overwrite existing partitions. Slot … may be
  unbootable until update finishes!`（我们 `ota.rs:65` 解析的就是这句，屏幕的「正在覆盖写入」
  = `Event::Overwriting`）。写完 `/metadata/ota` 不落 state，`what_is_left()`（`run.rs:648`）
  应当打 `snapshots None, merge outstanding false` —— **这正是与 A 组的差别，也是这次实验的
  主判据。**

## 2. 前置（有一条不满足就别开跑）

1. rec 是 v1.0.13：`grep -m1 'recovery up' /keel/run/recovery.log` 应带 `1.0.13`。
2. 电量 ≥ 15%（`short_of_charge` 会拒；系统请求的 retry 例外）。
3. `/metadata/ota/state` 不存在，或里面是 `None`（11-23 已核：这台机现在是干净的——`state` 0
   字节、`snapshots/` 空、`update_engine/` 不存在）。**若又出现残留的 `Snapshotted`，停在这里报我**
   ——那会以 `Refusal::Pending` 被当场拒（屏幕文案「更新已写入，等待合并。请重启到系统，完成上一次
   更新后重试。」），而且不要手删 `/metadata/ota` 里的东西。
4. `/proc/mounts` 里**没有** ` /data ` 这一项。有就说明这次 rec 已经把 data 开了（多半是上次
   待装请求触发的自动解密，或你去过「解密」页）：重启 rec 再看；仍开着就先在设置里关掉
   「记住凭证」再重启 rec。
5. A 槽可以牺牲。

**代价（提前讲清）**：覆盖写入会把 A 槽的 `boot_a` / `init_boot_a` / `vbmeta_a` / `dtbo_a` /
`vendor_boot_a` 和 super 里的 A 组分区直接换成包内原厂内容 —— 也就是说 **A 槽的 KernelSU
会没**（`init_boot_a` 被原厂镜像覆盖），实验后要重刷 KSU 补丁。`recovery_a` 也会被原厂覆写，
但 `keep_recovery=true` 会把它写回 1.0.13（`written back to [A]` 那行）。

## 3. 开跑前记录（原样复制，输出贴回来）

```sh
cat /proc/mounts | grep -E ' /(data|metadata|cache) ' ; echo ---
ls -l /metadata/ota ; echo ---
cat /metadata/ota/state 2>/dev/null | od -An -tx1 | head -6 ; echo ---
ls -l /metadata/ota/snapshots 2>/dev/null ; echo ---
getprop ro.boot.slot_suffix
grep -m1 'recovery up' /keel/run/recovery.log ; echo ---
for p in boot_a init_boot_a vbmeta_a dtbo_a vendor_boot_a recovery_a recovery_b ; do
  printf '%-14s ' $p ; sha256sum /dev/block/by-name/$p | cut -c1-16
done
```

（这几样是「装前基线」。装完逐条对照，尤其 `/metadata/ota` 和分区哈希。）

## 4. 步骤

手机上：

1. 主页 →「安装」→ 来源选 **「在电脑上运行 adb sideload」**（`pages.rs:1088` 那一行）。
2. 页面会显示「从 adb 调试切换到 sideload 模式」和 `adb sideload <file>` 的提示；按它切过去。
3. 切之前（或切之前的最后一步）再确认一次 `/proc/mounts` 里没有 ` /data `。

电脑上（逐条）：

```powershell
adb devices          # 应看到一行 <serial>  sideload
adb sideload "D:\桌面\玩机\mix flip 2\bao\bixi-ota_full-OS4.0.4.0.XOHCNXM-user-17.0-a6ba7e3e2c.zip"
```

- 这份包 `9432900450` 字节，USB 上传要几十分钟（2.0 口更久）。**中途不要拔线**，屏幕上写着
  「读取完成前，请不要断开数据线」。
- 手机上会出现确认页（`waits()` 对 Computer 来源只在 `wipe` 时才等确认；这份包无 `ota-wipe`，
  所以可能直接开装）→ 进度条。
- **看到「正在覆盖写入」的字样就对了**，那是 `Event::Overwriting`，说明引擎走的是 overwrite
  而不是快照。

## 5. 装完看这五处（判据）

1. `tail -20 /keel/run/recovery.log`，期望四行（顺序固定）：

```
install: an update to 17OS4.0.261113.004012506.QCPECN.S
install: left for the next start: snapshots None, merge outstanding false
install: this recovery written back to [A]: Ok(())
install: Installed { slot: A, wipe: false }
```

   **主判据是第二行**：`snapshots None, merge outstanding false`。A 组（11-22）那一行是
   `Snapshotted, merge outstanding true`。

2. `grep -n 'Will overwrite existing partitions' /keel/run/update_engine.log` —— 应有命中。
3. `ls -l /metadata/ota ; cat /metadata/ota/state` —— 装完应**没有** state 文件、也没有
   `snapshots/` 目录（对照 A 组：74 字节 state + 9 个目录）。
4. `grep -E 'Hashing partition|Error applying update' /keel/run/update_engine.log | tail` ——
   回读校验有走到、没有错误码。
5. **重启之前先拷现场**（红线：拷完、核对拷全，才允许重启）。完整脚本在
   `docs/rebase-4.0.4.0.md` §7.5，这里至少把这几样弄走：

```sh
OUT=/data/local/tmp/skyward-B
mkdir -p $OUT
cp /keel/run/recovery.log $OUT/ ; cp /keel/run/update_engine.log $OUT/
cp /metadata/ota/state $OUT/ 2>/dev/null ; od -An -tx1 /metadata/ota/state > $OUT/state.hex 2>/dev/null
ls -laR /metadata/ota/snapshots > $OUT/snapshots.ls 2>/dev/null
dmesg > $OUT/dmesg.txt ; cp /proc/mounts $OUT/mounts.txt
for p in boot_a init_boot_a vbmeta_a dtbo_a vendor_boot_a recovery_a ; do
  printf '%-14s ' $p ; sha256sum /dev/block/by-name/$p | cut -c1-16
done > $OUT/partitions.txt
ls -lR $OUT > $OUT/manifest.txt
```

   然后重启（不要进 fastboot），看首启 —— 进得去 / 起来又倒。**第一次倒之后就进 rec 拷现场**
   （清单在 `docs/rebase-4.0.4.0.md` §7.5），不必等它回滚；再按 §7.4 的证据源取证（**pstore 在这台
   机上已确认空，别再去翻**）。

## 6. 失败分支怎么判

| 现象 | 意思 | 下一步 |
| --- | --- | --- |
| 手机上直接拒：「更新已写入，等待合并…」 | `/metadata/ota/state` 有残留（`Refusal::Pending`） | 停，报我，先解决残留 |
| 手机上拒：「电量仅剩…」 | < 15% | 充电后重来 |
| PC 侧 `adb sideload` 报 `failed to read command` / `protocol fault` | 传到一半断链 | 贴 PC 输出 + `tail /keel/run/recovery.log` |
| 引擎在 Prepare 阶段直接报错（不是 overwrite） | `" /data 关着"` 这条路引擎不接受 | 也是一个有效结论，贴 `update_engine.log` 最后 40 行 |
| 首启起来又倒 | 见 §0 第二行 | 第一次倒就进 rec：先看 `/metadata/ota` 的 state（§7.2 判据），再把 §7.5 的清单拷走 |

## 7. 收尾

- **进得了系统**：A 槽是干净的 4.0.4.0；重刷 KSU 补丁（`init_boot_a` 已被原厂镜覆盖）；
  核对 `recovery_a` 回到 `3676c676…4aa06`（`written back` 那行是它的证据）。
- **起来又倒**：先进 rec 看 `/metadata/ota` 的 state 与 `snapshots/`（两条判据在 §7.2），再重刷 A 槽
  （有先例）。卡住那段的内核日志不会被 pstore 留下，别等它。
- 把 §8 的表填了贴我。

## 8. 记录表（填完贴回）

| 项 | 装前 | 装后 |
| --- | --- | --- |
| `/proc/mounts` 里有没有 ` /data ` | | |
| `/metadata/ota/state` 大小 / 内容 | | |
| `/metadata/ota/snapshots/` 条数 | | |
| `boot_a` 前 16 位 sha | | |
| `init_boot_a` 前 16 位 sha | | |
| `recovery_a` 前 16 位 sha | | |
| `recovery.log` 的 `left for the next start` 行 | | |
| `update_engine.log` 有没有 `Will overwrite` | | |
| 引擎退出码 / `Installed` 行 | | |
| 首启结果（进系统 / 起来又倒，几次、每次多久） | | |
