#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]

use defmt::*;
use embassy_executor::Spawner;
use embassy_stm32::adc::{Adc, SampleTime};
use embassy_stm32::peripherals::ADC;
use embassy_stm32::{adc, bind_interrupts};
use embassy_time::{Delay, Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    ADC1_COMP => adc::InterruptHandler<ADC>;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());
    info!("Hello World!");

    let mut adc = Adc::new(p.ADC, Irqs, &mut Delay);
    adc.set_sample_time(SampleTime::Cycles71_5);
    let mut pin = p.PA1;

    let mut vrefint = adc.enable_vref(&mut Delay);
    let vrefint_sample = adc.read_internal(&mut vrefint).await;
    let convert_to_millivolts = |sample| {
        // From https://www.st.com/resource/en/datasheet/stm32f031c6.pdf
        // 6.3.4 Embedded reference voltage
        const VREFINT_MV: u32 = 1230; // mV

        (u32::from(sample) * VREFINT_MV / u32::from(vrefint_sample)) as u16
    };

    loop {
        let v = adc.read(&mut pin).await;
        info!("--> {} - {} mV", v, convert_to_millivolts(v));
        Timer::after(Duration::from_millis(100)).await;
    }
}
