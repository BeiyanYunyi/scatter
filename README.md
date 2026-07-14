# Scatter

一个使用 Rust、wgpu 与 WGSL 实时绘制天空的桌面程序。它读取系统日期时间与目标经纬度，计算太阳高度角与方位角，并以文章 [On Rendering the Sky, Sunsets, and Planets](https://blog.maximeheckel.com/posts/on-rendering-the-sky-sunsets-and-planets/) 中的思路进行逐像素大气光线步进。

默认画面使用透视摄像机并自动对准太阳。方向键上下以 1° 步长控制 0°–90° 俯仰角，左右控制水平旋转；首次操作会进入手动镜头模式，按 `R` 恢复跟踪太阳。跟踪时以地平线为俯仰下限，太阳落到地平线下后镜头继续看向对应方位的地平线。

透视摄像机默认使用接近人眼自然观感的 50 mm 标准焦距（以 24 mm 画幅高度计）。使用鼠标滚轮可以在 12–300 mm 间连续调节焦距；调整窗口宽高比会直接改变画幅，垂直视场保持不变，窗口变宽时能看到更多横向天空。

除此之外，还有等距柱状投影方式。把配置文件中的 `rendering.projection` 改为 `equirectangular` 后，画面会变为完整的 360° 地平线展开图：正上方是天顶，底部略低于地平线，水平方向从南向西、北、东再回到南。该模式的天空画幅固定为 360:95，调整窗口大小时会自动居中并添加黑色留边，使两个方向具有相同的每像素角度。

程序会自动检查交换链能力：支持时优先使用 16-bit float 的线性 scRGB/EDR 输出，让太阳、太阳辉光和明亮地平线保留超过 SDR 白色的亮度；否则自动回退到 sRGB 交换链与 ACES 色调映射。窗口标题中的 `HDR` 或 `SDR` 会显示当前采用的输出模式。实际 HDR 亮度仍取决于显示器、操作系统 HDR 设置与桌面合成器。

## 运行

需要支持 wgpu 的 GPU 与 Rust 1.87 或更高版本：

```bash
cargo run --release
```

在 macOS 上可以把实时画面作为桌面背景运行：

```bash
cargo run --release -- --wallpaper
```

壁纸模式会为每块显示器创建一个覆盖全屏、位于 Finder 桌面图标下方的无边框窗口；
连接、断开显示器或调整排列后，会在下一次刷新时自动重建对应窗口。窗口不接收鼠标和
键盘事件，并以每秒一帧继续更新。它不会替换系统壁纸，也不会出现在锁屏或登录界面；
从终端按 `Control-C` 或结束 Scatter 进程即可退出。HDR surface 可用时，每块显示器的
`CAMetalLayer` 都会配置 extended-linear Display P3 色彩空间并使用 macOS EDR 输出。

按 `,`、`.` 以一分钟为步长后退、前进目标时间（按键重复有 50 ms 防抖），按 `T` 恢复当前时间。窗口标题显示由目标经度直接换算的地方平时（LMT），而不是行政时区时间。按 `Esc` 或关闭窗口退出。

## 配置

程序默认读取当前目录的 `config.toml`，也可以显式指定其它路径：

```bash
cargo run --release -- --config /path/to/scatter.toml
```

完整配置示例：

```toml
[location]
latitude = 31.2304
longitude = 121.4737

[rendering]
projection = "perspective"
force_sdr = false

[refresh]
reload_check_interval_ms = 500
```

配置文件会在运行期间自动检查和热重载。经纬度、投影方式或 SDR 选项变化后，程序会构建新的窗口与渲染器；只有构建成功才会替换当前画面。保存过程中出现不完整或不合法的 TOML 时会继续使用上一份有效配置，等待文件内容再次变化。`projection` 仅接受 `perspective`（默认）或 `equirectangular`；滚轮焦距调节只在透视模式下生效。

默认情况下，程序会在 surface 支持时优先启用 HDR/EDR。若需要强制使用 8-bit SDR surface（包括壁纸模式下的每块显示器），把 `rendering.force_sdr` 设为 `true`。

程序默认使用系统 UTC 偏移对应的标准经线，并以北纬 35° 作为代表性纬度。为了让日出、日落时间与所在地一致，请在 `location` 中设置实际坐标（东经、北纬为正）。省略 `longitude` 时仍会使用系统 UTC 偏移对应的标准经线。

## 模型

- CPU 端使用 NOAA fractional-year 近似式，从目标经度的地方平时与经纬度得到太阳高度角和真北方位角。
- 透视投影在跟踪模式下根据太阳方位自动生成摄像机朝向，并把俯仰角限制在 0°–90°；方向键切换到手动朝向后仍使用相同俯仰边界。针孔相机射线会随窗口宽高比无拉伸地扩展画幅。
- GPU 端在观察光线上累积 Rayleigh 与 Mie 密度，并沿太阳方向做嵌套光线步进，以 Beer–Lambert 定律计算透射率。
- Rayleigh 散射产生蓝色天空；Mie 前向散射产生太阳附近和地平线附近的暖色辉光；臭氧层作为波长相关吸收项参与光学深度。
- HDR surface 可用时，以线性 scRGB 输出并保留最高约 4 倍 SDR 白色的高光；不可用时使用 ACES 近似色调映射输出到 sRGB 交换链。

这是面向地面观察者的平面大气近似，并非行星尺度的球形大气或完整天文星历。它优先保证实时性、结构清晰和一天中光照变化的可信观感。

## 恒星

夜空会展示视星等不大于 6.5 的 8,920 颗恒星。程序根据观测时间、经度与纬度将
J2000 赤道坐标实时转换为地平坐标，并根据视星等、B-V 色指数、太阳高度及近地平线
大气消光控制星点的大小、颜色和可见度。J2000 坐标会随日期进行岁差修正；当前有意
忽略恒星自行与章动。

恒星目录位于 `data/stars`，由 `include_dir` 直接嵌入可执行文件，运行时不需要外部
数据文件或网络访问。该目录是 HYG Database v4.1 的筛选版本，并依照 CC BY-SA 4.0
单独分发；完整署名和修改说明见 `data/stars/README.md` 与 `data/stars/LICENSE.md`。
