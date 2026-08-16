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
};
use embedded_hal_bus::spi::ExclusiveDevice;

use cardputer_rust::{config, net, terminal, ui, wifi};

use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterruptControl,
    timer::timg::TimerGroup,
    delay::Delay,
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
};

use tca8418::{PinMask, Tca8418};

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

    // 起動直後に前回の残像（GRAM に残る内容）を消しておく。
    display.clear(Rgb565::BLACK).unwrap();

    // キーボード (TCA8418) を I2C0 で初期化。
    // SDA = GPIO8, SCL = GPIO9
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO8)
    .with_scl(peripherals.GPIO9);

    let mut keypad = Tca8418::new(i2c);
    keypad.configure_keypad(PinMask::ALL).unwrap();

    info!("Keyboard initialized");

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

    // ---- Wi-Fi 接続（リトライ付き）----
    const WIFI_MAX_RETRIES: u32 = 5;

    let mut connected = false;

    for attempt in 1..=WIFI_MAX_RETRIES {
        info!(
            "Connecting to {} ({}/{})...",
            config::WIFI_SSID,
            attempt,
            WIFI_MAX_RETRIES
        );
        let _ = ui::show_message(&mut display, style, "Connecting...");

        match wifi_controller.connect_async().await {
            Ok(ap_info) => {
                info!("Wi-Fi connected! AP info: {:?}", ap_info);
                connected = true;
                break;
            }
            Err(e) => {
                info!("Wi-Fi connection failed: {:?}", e);
                let _ = ui::show_message(&mut display, style, "WiFi retry...");
                embassy_time::Timer::after_secs(2).await;
            }
        }
    }

    if connected {
        let wifi_device = interfaces.station;

        // DHCPを使うIPv4ネットワークスタック
        let net_config = embassy_net::Config::dhcpv4(Default::default());
        let mut resources = embassy_net::StackResources::<3>::new();

        let (stack, mut runner) = embassy_net::new(
            wifi_device,
            net_config,
            &mut resources,
            0x1234_5678,
        );

        info!("DHCP starting...");

        // runner.run()（ネットワーク駆動）と、HTTP→キーボードの処理を
        // 同じ executor 上で並行に走らせる。runner は戻らないので join で常駐。
        embassy_futures::join::join(runner.run(), async {
            // DHCP はハングしないようタイムアウト付きで待つ。
            const DHCP_TIMEOUT_SECS: u64 = 15;

            let dhcp = embassy_futures::select::select(
                stack.wait_config_up(),
                embassy_time::Timer::after_secs(DHCP_TIMEOUT_SECS),
            )
            .await;

            match dhcp {
                embassy_futures::select::Either::First(()) => {
                    info!("DHCP completed!");

                    if let Some(cfg) = stack.config_v4() {
                        info!("IPv4 config: {:?}", cfg);
                    }

                    // HTTP GET（best-effort: 失敗しても panic せず表示するだけ）
                    let mut rx_buffer = [0u8; 2048];
                    let mut tx_buffer = [0u8; 1024];

                    let mut socket = embassy_net::tcp::TcpSocket::new(
                        stack,
                        &mut rx_buffer,
                        &mut tx_buffer,
                    );

                    let mut request_buf = [0u8; 128];
                    let request = net::write_get_request(
                        &mut request_buf,
                        config::REQUEST_PATH,
                        config::SERVER_HOST,
                    );

                    let mut response = [0u8; 2048];

                    info!("HTTP GET {}", config::REQUEST_PATH);

                    match net::http_get(
                        &mut socket,
                        config::SERVER_IP,
                        config::SERVER_PORT,
                        request,
                        &mut response,
                    )
                    .await
                    {
                        Ok(len) => {
                            let body = net::extract_body(&response[..len]);

                            if let Ok(text) = core::str::from_utf8(body) {
                                let text = text.trim();
                                info!("HTTP body: {}", text);
                                let _ =
                                    ui::show_message(&mut display, style, text);
                            } else {
                                let _ = ui::show_message(
                                    &mut display,
                                    style,
                                    "Bad response",
                                );
                            }
                        }
                        Err(e) => {
                            info!("HTTP failed: {:?}", e);
                            let _ = ui::show_message(
                                &mut display,
                                style,
                                "Server error",
                            );
                        }
                    }
                }
                embassy_futures::select::Either::Second(()) => {
                    info!("DHCP timed out");
                    let _ =
                        ui::show_message(&mut display, style, "DHCP timeout");
                }
            }

            info!("Entering keyboard loop");

            // ネットワークを生かしたままキーボード入力へ。
            terminal::run_input(&mut display, &mut keypad, style).await;
        })
        .await;
    } else {
        // Wi-Fi に接続できなかった → オフラインでキーボードのみ動かす。
        info!("Wi-Fi unavailable; running offline");
        let _ = ui::show_message(&mut display, style, "Offline");
        terminal::run_input(&mut display, &mut keypad, style).await;
    }
}
