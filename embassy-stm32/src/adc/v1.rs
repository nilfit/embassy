use core::future::poll_fn;
use core::marker::PhantomData;
use core::task::Poll;

use embassy_hal_internal::into_ref;
use embedded_hal_02::blocking::delay::DelayUs;

use crate::adc::{Adc, AdcPin, Instance, InternalChannel, Resolution, SampleTime};
use crate::interrupt::typelevel::Interrupt;
use crate::peripherals::ADC;
use crate::{interrupt, Peripheral};

pub const VDDA_CALIB_MV: u32 = 3300;
pub const VREF_INT: u32 = 1230;

/// Interrupt handler.
pub struct InterruptHandler<T: Instance> {
    _phantom: PhantomData<T>,
}

impl<T: Instance> interrupt::typelevel::Handler<T::Interrupt> for InterruptHandler<T> {
    unsafe fn on_interrupt() {
        if T::regs().isr().read().eoc() {
            T::regs().ier().modify(|w| w.set_eocie(false));
        } else {
            return;
        }

        T::state().waker.wake();
    }
}

pub struct Vbat;
impl InternalChannel<ADC> for Vbat {}
impl super::sealed::InternalChannel<ADC> for Vbat {
    fn channel(&self) -> u8 {
        18
    }
}

pub struct Vref;
impl InternalChannel<ADC> for Vref {}
impl super::sealed::InternalChannel<ADC> for Vref {
    fn channel(&self) -> u8 {
        17
    }
}

pub struct Temperature;
impl InternalChannel<ADC> for Temperature {}
impl super::sealed::InternalChannel<ADC> for Temperature {
    fn channel(&self) -> u8 {
        16
    }
}

impl<'d, T: Instance> Adc<'d, T> {
    pub fn new(
        adc: impl Peripheral<P = T> + 'd,
        _irq: impl interrupt::typelevel::Binding<T::Interrupt, InterruptHandler<T>> + 'd,
        delay: &mut impl DelayUs<u32>,
    ) -> Self {
        into_ref!(adc);
        T::enable();
        T::reset();

        // Delay 1μs when using HSI14 as the ADC clock.
        //
        // Table 57. ADC characteristics
        // tstab = 14 * 1/fadc
        delay.delay_us(1);

        // A.7.1 ADC calibration code example
        T::regs().cfgr1().modify(|reg| reg.set_dmaen(false));
        T::regs().cr().modify(|reg| reg.set_adcal(true));
        while T::regs().cr().read().adcal() {}

        // A.7.2 ADC enable sequence code example
        if T::regs().isr().read().adrdy() {
            T::regs().isr().modify(|reg| reg.set_adrdy(true));
        }
        T::regs().cr().modify(|reg| reg.set_aden(true));
        while !T::regs().isr().read().adrdy() {
            // ES0233, 2.4.3 ADEN bit cannot be set immediately after the ADC calibration
            // Workaround: When the ADC calibration is complete (ADCAL = 0), keep setting the
            // ADEN bit until the ADRDY flag goes high.
            T::regs().cr().modify(|reg| reg.set_aden(true));
        }

        T::Interrupt::unpend();
        unsafe {
            T::Interrupt::enable();
        }

        Self {
            adc,
            sample_time: Default::default(),
        }
    }

    pub fn enable_vbat(&self, _delay: &mut impl DelayUs<u32>) -> Vbat {
        // SMP must be ≥ 56 ADC clock cycles when using HSI14.
        //
        // 6.3.20 Vbat monitoring characteristics
        // ts_vbat ≥ 4μs
        T::regs().ccr().modify(|reg| reg.set_vbaten(true));
        Vbat
    }

    pub fn enable_vref(&self, delay: &mut impl DelayUs<u32>) -> Vref {
        // Table 28. Embedded internal reference voltage
        // tstart = 10μs
        T::regs().ccr().modify(|reg| reg.set_vrefen(true));
        delay.delay_us(10);
        Vref
    }

    pub fn enable_temperature(&self, delay: &mut impl DelayUs<u32>) -> Temperature {
        // SMP must be ≥ 56 ADC clock cycles when using HSI14.
        //
        // 6.3.19 Temperature sensor characteristics
        // tstart ≤ 10μs
        // ts_temp ≥ 4μs
        T::regs().ccr().modify(|reg| reg.set_tsen(true));
        delay.delay_us(10);
        Temperature
    }

    pub fn set_sample_time(&mut self, sample_time: SampleTime) {
        self.sample_time = sample_time;
    }

    pub fn set_resolution(&mut self, resolution: Resolution) {
        T::regs().cfgr1().modify(|reg| reg.set_res(resolution.into()));
    }

    pub async fn read<P>(&mut self, pin: &mut P) -> u16
    where
        P: AdcPin<T> + crate::gpio::sealed::Pin,
    {
        let channel = pin.channel();
        pin.set_as_analog();
        self.read_channel(channel).await
    }

    pub async fn read_internal(&mut self, channel: &mut impl InternalChannel<T>) -> u16 {
        let channel = channel.channel();
        self.read_channel(channel).await
    }

    async fn convert(&mut self) -> u16 {
        T::regs().isr().modify(|reg| {
            reg.set_eoc(true);
            reg.set_eosmp(true);
        });

        T::regs().smpr().modify(|reg| reg.set_smp(self.sample_time.into()));
        T::regs().ier().modify(|w| w.set_eocie(true));
        T::regs().cr().modify(|reg| reg.set_adstart(true));

        poll_fn(|cx| {
            T::state().waker.register(cx.waker());

            if T::regs().isr().read().eoc() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;

        T::regs().dr().read().data()
    }

    async fn read_channel(&mut self, channel: u8) -> u16 {
        // A.7.5 Single conversion sequence code example - Software trigger
        T::regs().chselr().write(|reg| reg.set_chselx(channel as usize, true));

        self.convert().await
    }
}

impl<'d, T: Instance> Drop for Adc<'d, T> {
    fn drop(&mut self) {
        // A.7.3 ADC disable code example
        T::regs().cr().modify(|reg| reg.set_adstp(true));
        while T::regs().cr().read().adstp() {}

        T::regs().cr().modify(|reg| reg.set_addis(true));
        while T::regs().cr().read().aden() {}

        T::disable();
    }
}
