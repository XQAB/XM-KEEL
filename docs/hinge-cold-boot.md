# 冷启动进 recovery 就能读 hinge_angle：配方、读数、以及重力为什么不可得

> 机型 bixi（MIX Flip 2，SM8750）。这份文档只讲**从 Android 冷启动进 recovery、一条命令都不敲**
> 的那条路径：谁把 ADSP 的 sensor 服务拉起来、第一帧有没有被挡、hinge 出值什么样，以及为什么
> 这个环境里没有重力可用。分区的挂载、linker 命名空间、daemon 顺序的来龙去脉在
> [`sensors-in-recovery.md`](sensors-in-recovery.md)；实现在
> [`crates/keel/src/sensors.rs`](../crates/keel/src/sensors.rs)（六步）与
> [`crates/keel-sensors`](../crates/keel-sensors)（QRTR 客户端）。

## 1. 冷启动时没人铺环境

空白冷启动（uptime 56 秒、什么都没做）时的样子：

| 项 | 值 |
| --- | --- |
| `/sys/class/remoteproc/remoteproc1/state` | `running` |
| `/proc/misc` 里的 fastrpc 项 | 空 |
| `/dev/fastrpc-adsp-secure` | 不存在 |
| `/linkerconfig/ld.config.txt` | 356 字节（只有 `namespace.default`） |
| `pd-mapper`／`adsprpcd`／`sscrpcd` | 一个都没有 |
| `/vendor/dsp`／`/persist` | 未挂载 |

这时 `hinge_angle` 一个事件都没有。kernel 已经把 `remoteproc-adsp` 上电了（`powering up`），
但 recovery 的 init 不会挂 `dsp_a`、不 insmod `frpc-adsprpc.ko`、不写 vendor 的 linker 命名空间、
也不起那三个 daemon——这些是 `bring_up()` 自己做的六步（细节见
[`sensors-in-recovery.md` §5.1](sensors-in-recovery.md)）。

**不要写 `/sys/kernel/boot_adsp/ssr`。** 六步全做、完全不碰 ssr，registry 2 秒到 140、5 秒到 142、
60 秒仍是 142，`hinge_angle` 正常出值；写了也一样。那个节点是音频子系统挂死后的 ADSP 重启开关。

两条容易踩错的：

- **minor 每次启动都会变**（见过 281 和 282）。节点必须按 `/proc/misc` 读出来的 minor 去建，
  不能写死、也不能「不存在就 mknod 别管 minor」；
- **不起 daemon 就只有 31 个 uid。** adb 掉线会用重连把前台脚本打断，看起来像「写了 ssr 之后
  daemon 全死」；用 `setsid`／`nohup` 脱离 adb 跑，三个 daemon 一个都不死。

## 2. 一次真机冷启动的读数（1.0.0 预发布镜像，A 槽）

```
[ 3.460] skyward: persist is mounted on /mnt/vendor/persist
[10.278] set 51 backlight 200                     ← 首帧画出
[10.667] skyward: sensors: bringing the sensor service up
[10.674] skyward: sensors: /dev/block/by-name/dsp_a mounted as ext4
[10.684] skyward: sensors: frpc-adsprpc is loaded
[10.684] skyward: sensors: /dev/fastrpc-adsp-secure made, minor 281
[10.684] skyward: sensors: the linker namespace is written
[10.690] skyward: sensors: pd-mapper started as 757
[11.701] skyward: sensors: adsprpcd started as 823
[13.724] skyward: sensors: sscrpcd started as 836
[15.745] skyward: sensors: the sensor service answers
```

- 首帧 `10.278` **早于** bring_up `10.667`：六步在独立线程里，首页不等人（同步版会把首帧拖到
  十五秒以后）；
- registry 到 **142 uids**，与手测满值一致；
- 一次折动的 209 个样本（`crates/keel-sensors/examples/hingewatch.rs`，0.02 s → 62.88 s）：

| 时刻 | 角度 | |
| --- | --- | --- |
| `35.58s` | `178°` | 开始合 |
| `36.42s` | `89°` | 半折 |
| `36.93s` | `16°` | |
| `37.21s` | `0°` | 完全合上（83 样本） |
| `39.57s` | `2°` | 开始展开（中间 2.4 秒静止，on-change 不发样本） |
| `49.31s` | `167°` | |
| `62.88s` | `178°` | 完全展开（118 样本） |

滞回 `FOLDED_BELOW=20`／`OPEN_ABOVE=150` 在这批数据上判得干净。

**外屏切换不依赖这条链路。** 零改动冷启动、三个 daemon 一个都没起时，触摸驱动的
`capfold_status` 已经在跟着折动变（合上 `0` + `p0=0 p1=819`，展开 `1` + `p0=819 p1=0`）。
`shut_now()` 是 `fold_status().or_else(|| flip.folded())`，hinge 只是第二判据；`bring_up()`
失败只写 kmsg、不挡启动，也绝不能写成换屏的前置条件。

## 3. 重力翻转在 recovery 里不可行（实测结论）

六个候选逐个用 `enable(suid, 10)`（514，on-change）打：

| 传感器 | 结果 |
| --- | --- |
| `accel`（`3846c24e` lsm6dsv、`3c4b340b` lsm6dso） | `ack` 之后 `status 130, code 5`；512 三条 `error 7`、部分无答案 |
| `gravity` | `status 130, code 2`（两个 uid） |
| `gyro` | `status 130, code 3`（两个 uid） |
| `orientation`（`ori`） | `status 130, code 6` |
| `tilt` | `514 acked but never answered`（三个 uid），0 样本 |
| `device_orient` | 能读，但 10 分钟主动旋转只出 1 条固定 `[4, 4, 0 …]`，后续 payload 全 0 |

`accel` 不是「没认出来」：属性表是完整的 650 字节（`sns_accel.proto`、rates 30/60/120/240/480、
`stream_type 0` continuous、量程 960 g）。卡在 config 的内层 msg id——既不是
`sns_std_sensor_config` 的 512，也不是 513／514，不在 `libsnsapi` 的标准枚举里；4 个 uid × 6 种
组合全是拒绝或只回状态事件，零真样本。recovery 与 Android 两侧一致。

另外两条也不成立：

- `screen_down` 只认「屏幕朝下放桌上」，物理旋转不响应；
- 内核直读没有出口：recovery 里没有 `i2c-dev.ko`／`spidev.ko`，`/sys/class/sensors` 不存在。

**所以：不做按重力的自动 180° 翻转。** 页面朝向只有触摸驱动的 `capfold_status` 与 ADSP 上的
`hinge_angle` 两个来源，180° 翻转维持设置页的手动开关。代码里也照这个结论收干净了：
`crates/keel/src/orientation.rs` 只剩 `probe()`（`keelbin sns` 用），那套线程化的方向监听
（`start()`／`Sensor`／`Direction`）已删除，模块注释里写着上面这张表。
