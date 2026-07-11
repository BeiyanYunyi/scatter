# Scatter

一个使用 Rust、wgpu 与 WGSL 实时绘制天空的桌面程序。它读取系统日期、时间和时区，计算太阳高度角与方位角，并以文章 [On Rendering the Sky, Sunsets, and Planets](https://blog.maximeheckel.com/posts/on-rendering-the-sky-sunsets-and-planets/) 中的思路进行逐像素大气光线步进。

默认画面使用透视摄像机并自动对准太阳，不支持手动旋转。摄像机在垂直方向以地平线为死区下限：太阳落到地平线下后，摄像机继续看向对应方位的地平线，俯仰角不会小于 0°，因而不会转向地面。

透视摄像机默认使用接近人眼自然观感的 50 mm 标准焦距（以 24 mm 画幅高度计）。使用鼠标滚轮可以在 12–300 mm 间连续调节焦距；调整窗口宽高比会直接改变画幅，垂直视场保持不变，窗口变宽时能看到更多横向天空。

原有等距柱状投影仍然保留。设置 `SKY_PROJECTION=equirectangular` 后，画面会变为完整的 360° 地平线展开图：正上方是天顶，底部略低于地平线，水平方向从南向西、北、东再回到南。该模式的天空画幅固定为 360:95，调整窗口大小时会自动居中并添加黑色留边，使两个方向具有相同的每像素角度。

程序会自动检查交换链能力：支持时优先使用 16-bit float 的线性 scRGB/EDR 输出，让太阳、太阳辉光和明亮地平线保留超过 SDR 白色的亮度；否则自动回退到 sRGB 交换链与原有的 ACES 色调映射。窗口标题中的 `HDR` 或 `SDR` 会显示当前采用的输出模式。实际 HDR 亮度仍取决于显示器、操作系统 HDR 设置与桌面合成器。

## 运行

需要支持 wgpu 的 GPU 与 Rust 1.87 或更高版本：

```bash
cargo run --release
```

按 `Esc` 或关闭窗口退出。

默认透视投影与原有展开投影的启动方式分别为：

```bash
cargo run --release
SKY_PROJECTION=equirectangular cargo run --release
```

`SKY_PROJECTION` 仅接受 `perspective`（默认）或 `equirectangular`。窗口标题会显示当前投影方式；滚轮焦距调节只在透视模式下生效。

程序默认使用系统 UTC 偏移对应的标准经线，并以北纬 35° 作为代表性纬度。为了让日出、日落时间与所在地一致，请设置实际坐标（东经、北纬为正）：

```bash
SKY_LATITUDE=31.2304 SKY_LONGITUDE=121.4737 cargo run --release
```

## 模型

- CPU 端使用 NOAA fractional-year 近似式，从系统本地时间、UTC 偏移、经纬度得到太阳高度角和真北方位角。
- 透视投影根据太阳方位自动生成摄像机朝向，将太阳高度角钳制在 0°–90°，并根据窗口宽高比建立无拉伸的针孔相机射线。
- GPU 端在观察光线上累积 Rayleigh 与 Mie 密度，并沿太阳方向做嵌套光线步进，以 Beer–Lambert 定律计算透射率。
- Rayleigh 散射产生蓝色天空；Mie 前向散射产生太阳附近和地平线附近的暖色辉光；臭氧层作为波长相关吸收项参与光学深度。
- HDR surface 可用时，以线性 scRGB 输出并保留最高约 4 倍 SDR 白色的高光；不可用时使用 ACES 近似色调映射输出到 sRGB 交换链。

这是面向地面观察者的平面大气近似，并非行星尺度的球形大气或完整天文星历。它优先保证实时性、结构清晰和一天中光照变化的可信观感。
