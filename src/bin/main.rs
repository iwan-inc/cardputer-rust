#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types"
)]
#![deny(clippy::large_stack_frames)]

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    text::Text,
};
use embedded_hal_bus::spi::ExclusiveDevice;

use cardputer_rust::{config, net, ui, wifi};

use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterruptControl,
    timer::timg::TimerGroup,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
};

use log::info;

use mipidsi::{
    Builder,
    interface::SpiInterface,
    models::ST7789,
    options::{
        ColorInversion,
        Orientation,
        Rotation,
    },
};

esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]

use embassy_executor::Spawner;

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default()
        .with_cpu_clock(CpuClock::max());

    let peripherals = esp_hal::init(config);

    // Wi-Fi用ヒープ
    esp_alloc::heap_allocator!(
        #[esp_hal::ram(reclaimed)]
        size: 64 * 1024
    );
    esp_alloc::heap_allocator!(size: 36 * 1024);

    // esp-radioにはRTOSスケジューラが必要
    let timg0 = TimerGroup::new(peripherals.TIMG0);

    let software_interrupt =
        SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);

    esp_rtos::start(
        timg0.timer0,
        software_interrupt.software_interrupt0,
    );

    info!("RTOS started");

    let controller_config =
        wifi::controller_config(config::WIFI_SSID, config::WIFI_PASSWORD);

    let (mut wifi_controller, interfaces) =
        esp_radio::wifi::new(
            peripherals.WIFI,
            controller_config,
        )
        .unwrap();

    info!("Wi-Fi initialized");

    // LCDバックライト
    let _backlight = Output::new(
        peripherals.GPIO38,
        Level::High,
        OutputConfig::default(),
    );

    // LCD制御用GPIO
    let dc = Output::new(
        peripherals.GPIO34,
        Level::Low,
        OutputConfig::default(),
    );

    let reset = Output::new(
        peripherals.GPIO33,
        Level::High,
        OutputConfig::default(),
    );

    let cs = Output::new(
        peripherals.GPIO37,
        Level::High,
        OutputConfig::default(),
    );

    // SPI
    // MOSI = GPIO35
    // SCK  = GPIO36
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(20))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO36)
    .with_mosi(peripherals.GPIO35);

    let spi_device = ExclusiveDevice::new_no_delay(spi, cs).unwrap();

    // mipidsi用バッファ
    let mut buffer = [0_u8; 512];

    let di = SpiInterface::new(
        spi_device,
        dc,
        &mut buffer,
    );

    let mut delay = Delay::new();

    let mut display = Builder::new(ST7789, di)
        .reset_pin(reset)
        .display_size(135, 240)
        .display_offset(52, 40)
        .orientation(
            Orientation::new()
                .rotate(Rotation::Deg90)
        )
        .invert_colors(ColorInversion::Inverted)
        .init(&mut delay)
        .unwrap();

    let style = ui::text_style();

    info!("LCD initialized");

    // 起動直後に前回の残像（GRAM に残る内容）を消し、
    // 通信中であることを表示しておく。
    display.clear(Rgb565::BLACK).unwrap();

    Text::new(
        "Connecting...",
        Point::new(20, 70),
        style,
    )
    .draw(&mut display)
    .unwrap();

    /*
    info!("Scanning Wi-Fi...");

    let scan_config =
        ScanConfig::default()
            .with_max(20);

    let networks = wifi_controller
        .scan_async(&scan_config)
        .await
        .unwrap();

    info!("Found {} networks", networks.len());

    for ap in networks {
        info!(
            "SSID: {}  RSSI: {}  CH: {}",
            ap.ssid.as_str(),
            ap.signal_strength,
            ap.channel
        );
    }

    info!("Wi-Fi scan complete");

    loop {
        embassy_time::Timer::after_secs(1).await;
    }
    */

    info!("Connecting to {}...", config::WIFI_SSID);

    match wifi_controller.connect_async().await {
        Ok(info) => {
            info!("Wi-Fi connected!");
            info!("AP info: {:?}", info);

            let wifi_device = interfaces.station;

            // DHCPを使うIPv4ネットワークスタック
            let net_config =
                embassy_net::Config::dhcpv4(Default::default());

            // DHCP用を含めてソケット領域を確保
            let mut resources =
                embassy_net::StackResources::<3>::new();

            let (stack, mut runner) = embassy_net::new(
                wifi_device,
                net_config,
                &mut resources,
                0x1234_5678,
            );

            info!("DHCP starting...");

            embassy_futures::join::join(
                runner.run(),
                async {
                    // DHCPでIPv4設定が取れるまで待つ
                    stack.wait_config_up().await;

                    info!("DHCP completed!");

                    if let Some(config) = stack.config_v4() {
                        info!("IPv4 config: {:?}", config);
                    }

                    // TCP送受信バッファ
                    let mut rx_buffer = [0u8; 2048];
                    let mut tx_buffer = [0u8; 1024];

                    let mut socket = embassy_net::tcp::TcpSocket::new(
                        stack,
                        &mut rx_buffer,
                        &mut tx_buffer,
                    );

                    info!("Connecting to Mac...");

                    socket
                        .connect((config::SERVER_IP, config::SERVER_PORT))
                        .await
                        .unwrap();

                    info!("Connected to Mac");

                    // HTTP GETを送信
                    let mut request_buf = [0u8; 128];
                    let request = net::write_get_request(
                        &mut request_buf,
                        config::REQUEST_PATH,
                        config::SERVER_HOST,
                    );

                    socket.write(request).await.unwrap();
                    socket.flush().await.unwrap();

                    info!("HTTP request sent");

                    // レスポンスを読む
                    let mut read_buffer = [0u8; 512];
                    let mut response = [0u8; 2048];
                    let mut response_len = 0;

                    loop {
                        let n = socket.read(&mut read_buffer).await.unwrap();

                        if n == 0 {
                            break;
                        }

                        let remaining = response.len() - response_len;
                        let copy_len = core::cmp::min(n, remaining);

                        response[response_len..response_len + copy_len]
                            .copy_from_slice(&read_buffer[..copy_len]);

                        response_len += copy_len;

                        if response_len == response.len() {
                            break;
                        }
                    }

                    let response_bytes = &response[..response_len];

                    let body = net::extract_body(response_bytes);

                    if let Ok(text) = core::str::from_utf8(body) {
                        let text = text.trim();

                        info!("HTTP body: {}", text);

                        display.clear(Rgb565::BLACK).unwrap();

                        Text::new(
                            text,
                            Point::new(20, 70),
                            style,
                        )
                        .draw(&mut display)
                        .unwrap();
                    }

                    info!("HTTP complete");

                    loop {
                        embassy_time::Timer::after_secs(1).await;
                    }
                },
            )
            .await;
        }
        Err(e) => {
            info!("Wi-Fi connection failed: {:?}", e);
        }
    }

    

    /*
    let i2c_config = I2cConfig::default()
        .with_frequency(Rate::from_khz(400));

    let i2c = I2c::new(
        peripherals.I2C0,
        i2c_config,
    )
    .unwrap()
    .with_sda(peripherals.GPIO8)
    .with_scl(peripherals.GPIO9);

    let mut keypad = Tca8418::new(i2c);

    // Cardputer Advは4行×14列ですが、
    // TCA8418側では最大8行×10列なので、まず全ピンを有効化してイベント確認
    keypad
        .configure_keypad(PinMask::ALL)
        .unwrap();

    info!("Keyboard initialized");

    info!("LCD test start");

    // LCDバックライト
    let _backlight = Output::new(
        peripherals.GPIO38,
        Level::High,
        OutputConfig::default(),
    );

    // LCD制御用GPIO
    let dc = Output::new(
        peripherals.GPIO34,
        Level::Low,
        OutputConfig::default(),
    );

    let reset = Output::new(
        peripherals.GPIO33,
        Level::High,
        OutputConfig::default(),
    );

    let cs = Output::new(
        peripherals.GPIO37,
        Level::High,
        OutputConfig::default(),
    );

    // SPI
    // MOSI = GPIO35
    // SCK  = GPIO36
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(20))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(peripherals.GPIO36)
    .with_mosi(peripherals.GPIO35);

    let spi_device = ExclusiveDevice::new_no_delay(spi, cs).unwrap();

    // mipidsi用バッファ
    let mut buffer = [0_u8; 512];

    let di = SpiInterface::new(
        spi_device,
        dc,
        &mut buffer,
    );

    let mut delay = Delay::new();

    let mut display = Builder::new(ST7789, di)
        .reset_pin(reset)
        .display_size(135, 240)
        .display_offset(52, 40)
        .orientation(
            Orientation::new()
                .rotate(Rotation::Deg90)
        )
        .invert_colors(ColorInversion::Inverted)
        .init(&mut delay)
        .unwrap();

    info!("LCD initialized");

    display.clear(Rgb565::BLACK).unwrap();

    let style = MonoTextStyle::new(
        &FONT_10X20,
        Rgb565::WHITE,
    );

    let mut x = 10;
    let mut y = 40;

    const CHAR_W: i32 = 10;
    const CHAR_H: i32 = 20;

    const LEFT: i32 = 10;
    const RIGHT: i32 = 230;
    const TOP: i32 = 40;
    const BOTTOM: i32 = 125;

    Text::new(
        "Hello Rust!",
        Point::new(20, 70),
        style,
    )
    .draw(&mut display)
    .unwrap();

    info!("LCD painted BLACK with text");

    let mut shift_down = false;

    loop {
        for event in keypad.events().unwrap() {
            if let Some(key) = event.pressed_keypad() {
                let (row, col) = remap_key(key.row, key.col);

                match (row, col) {
                    // Shift
                    (2, 1) => {
                        shift_down = true;
                        info!("SHIFT DOWN");
                    }

                    // Backspace
                    (0, 13) => {
                        if x > LEFT {
                            x -= CHAR_W;

                            Rectangle::new(
                                Point::new(x, y - CHAR_H + 4),
                                Size::new(CHAR_W as u32, CHAR_H as u32),
                            )
                            .into_styled(
                                PrimitiveStyle::with_fill(Rgb565::BLACK)
                            )
                            .draw(&mut display)
                            .unwrap();
                        }

                        info!("BACKSPACE");
                    }

                    // Enter
                    (2, 13) => {
                        x = LEFT;
                        y += CHAR_H;

                        info!("ENTER");
                    }

                    (1, 0) => info!("TAB"),
                    (2, 0) => info!("FN"),
                    (3, 0) => info!("CTRL"),
                    (3, 1) => info!("OPT"),
                    (3, 2) => info!("ALT"),

                    _ => {
                        if let Some(ch) = key_to_char(key.row, key.col) {
                            let ch = if shift_down {
                                shift_char(ch)
                            } else {
                                ch
                            };

                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);

                            Text::new(
                                s,
                                Point::new(x, y),
                                style,
                            )
                            .draw(&mut display)
                            .unwrap();

                            x += CHAR_W;

                            // 右端まで来たら自動改行
                            if x > RIGHT - CHAR_W {
                                x = LEFT;
                                y += CHAR_H;
                            }

                            info!("KEY {}", ch);
                        }
                    }
                }
            }

            if let Some(key) = event.released_keypad() {
                let (row, col) = remap_key(key.row, key.col);

                if (row, col) == (2, 1) {
                    shift_down = false;
                    info!("SHIFT UP");
                }
            }
        }
        
    }
    */
}
