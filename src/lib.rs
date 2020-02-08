//! X-Powers AXP173 Power Management IC
//! Datasheet: [/doc/AXP173 Datasheet v1.12_cn.zh-CN.en.pdf]

#![no_std]
#![deny(missing_docs)]
#![deny(unsafe_code)]

use embedded_hal::blocking::i2c::{Write, WriteRead};

use bit_field::BitField;

mod adc;
mod ldo;
mod regs;
mod units;

use regs::*;
use units::*;

pub use adc::AdcSettings;
pub use ldo::{Ldo, LdoKind};
pub use regs::{AdcSampleRate, ChargingCurrent, ChargingVoltage, TsPinMode};

/// AXP173 I2C address
/// 7-bit: 0x34
const AXP173_ADDR: u8 = 0x68;

/// Default values for the on-chip buffer.
/// Datasheet, p. 28, section 9.11.
const AXP173_ON_CHIP_BUFFER_DEFAULT: [u8; 6] = [0xf0, 0x0f, 0x00, 0xff, 0x00, 0x00];

type Axp173Result<T, E> = Result<T, Error<E>>;
type OperationResult<E> = Axp173Result<(), E>;

/// All possible errors in this crate
#[derive(Debug)]
pub enum Error<E> {
    /// I2C bus error
    I2c(E),

    /// Communication failure or invalid chip used
    InvalidChip([u8; 6]),
}

/// AXP173 PMIC instance.
pub struct Axp173<I> {
    i2c: I,
}

impl<I, E> Axp173<I>
where
    I: WriteRead<Error = E> + Write<Error = E>,
{
    /// Side-effect-free constructor.
    /// Nothing will be read or written before `init()` call.
    pub fn new(i2c: I) -> Self {
        Axp173 { i2c }
    }

    /// Checks the I2C connection to the AXP173 chip.
    /// AXP173 doesn't have a dedicated chip ID register and connection is checked by reading
    /// on-chip buffer for a presence of default values.
    pub fn init(&mut self) -> OperationResult<E> {
        self.read_u8(POWER_DATA_BUFFER1).map_err(Error::I2c)?;

        let buf = self.read_onchip_buffer()?;

        match buf {
            AXP173_ON_CHIP_BUFFER_DEFAULT => Ok(()),
            _ => Err(Error::InvalidChip(buf)),
        }
    }

    /// Reads 6-byte user data buffer from the chip.
    pub fn read_onchip_buffer(&mut self) -> Axp173Result<[u8; 6], E> {
        let mut buf = [0u8; 6];
        self.read_bytes(POWER_DATA_BUFFER1, &mut buf)
            .map_err(Error::I2c)?;

        Ok(buf)
    }

    /// Writes 6-byte user data buffer into the chip.
    #[allow(clippy::trivially_copy_pass_by_ref)] // On 32-bit ARMs it is not efficient to pass by value
    pub fn write_onchip_buffer(&mut self, bytes: &[u8; 6]) -> OperationResult<E> {
        let mut buf = [0u8; 6 + 1];

        buf[0] = POWER_DATA_BUFFER1;
        for (i, byte) in bytes.iter().enumerate() {
            buf[i + 1] = *byte;
        }

        self.i2c.write(AXP173_ADDR, &buf[..]).map_err(Error::I2c)?;

        Ok(())
    }

    /// Returns `true` if device is connected to the USB power source.
    pub fn vbus_present(&mut self) -> Axp173Result<bool, E> {
        let reg_val = self.read_u8(POWER_STATUS).map_err(Error::I2c)?;

        Ok(reg_val.get_bit(POWER_STATUS_VBUS_PRESENT))
    }

    /// Returns `true` if lithium battery is connected.
    pub fn battery_present(&mut self) -> Axp173Result<bool, E> {
        let reg_val = self.read_u8(POWER_MODE_CHGSTATUS).map_err(Error::I2c)?;

        Ok(reg_val.get_bit(POWER_MODE_CHGSTATUS_BATTERY_PRESENT))
    }

    /// Returns `true` if lithium battery is connected and currently charging.
    pub fn battery_charging(&mut self) -> Axp173Result<bool, E> {
        let reg_val = self.read_u8(POWER_MODE_CHGSTATUS).map_err(Error::I2c)?;

        Ok(reg_val.get_bit(POWER_MODE_CHGSTATUS_IS_CHARGING))
    }

    /// Enables selected LDO with selected output voltage.
    pub fn enable_ldo(&mut self, ldo: &Ldo) -> OperationResult<E> {
        self.set_ldo_voltage(&ldo)?;
        self.switch_ldo(&ldo.kind, true)
    }

    /// Disables selected LDO.
    pub fn disable_ldo(&mut self, ldo: &LdoKind) -> OperationResult<E> {
        self.switch_ldo(ldo, false)
    }

    fn switch_ldo(&mut self, ldo: &LdoKind, enable: bool) -> OperationResult<E> {
        let bit = match ldo {
            LdoKind::LDO2 => POWER_ON_OFF_REG_LDO2_ON,
            LdoKind::LDO3 => POWER_ON_OFF_REG_LDO3_ON,
            LdoKind::LDO4 => POWER_ON_OFF_REG_LDO4_ON,
        };

        let mut bits = self.read_u8(POWER_ON_OFF_REG).map_err(Error::I2c)?;

        bits.set_bit(bit, enable);

        self.write_u8(POWER_ON_OFF_REG, bits).map_err(Error::I2c)?;

        Ok(())
    }

    fn set_ldo_voltage(&mut self, ldo: &Ldo) -> OperationResult<E> {
        let reg = match &ldo.kind {
            LdoKind::LDO2 | LdoKind::LDO3 => LDO2_LDO3_OUT_VOL_REG,
            LdoKind::LDO4 => LDO4_OUT_VOL_REG,
        };

        let voltage = ldo.voltage;

        let mut bits = self.read_u8(reg).map_err(Error::I2c)?;

        let bits_range = match &ldo.kind {
            LdoKind::LDO2 => 4..8,
            LdoKind::LDO3 => 0..4,
            LdoKind::LDO4 => 0..7,
        };

        bits.set_bits(bits_range, voltage);

        self.write_u8(reg, bits).map_err(Error::I2c)?;

        Ok(())
    }

    /// Sets charging current of the battery. Adjust this for an efficient and safe charging of
    /// your lithium battery's capacity.
    pub fn set_charging_current(&mut self, current: ChargingCurrent) -> OperationResult<E> {
        let mut bits = self.read_u8(POWER_CHARGE1).map_err(Error::I2c)?;
        bits.set_bits(POWER_CHARGE1_CURRENT_SETTING, current.bits());

        self.write_u8(POWER_CHARGE1, bits).map_err(Error::I2c)?;

        Ok(())
    }

    /// Sets battery charging regulation voltage. This value varies depending on battery's chemistry
    /// and should be as close to the value from battery's datasheet as possible to ensure
    /// efficient charging and long battery life.
    pub fn set_charging_voltage(&mut self, voltage: ChargingVoltage) -> OperationResult<E> {
        let mut bits = self.read_u8(POWER_CHARGE1).map_err(Error::I2c)?;
        bits.set_bits(POWER_CHARGE1_VOLTAGE_SETTING, voltage.bits());

        self.write_u8(POWER_CHARGE1, bits).map_err(Error::I2c)?;

        Ok(())
    }

    /// Enables or disables battery charging.
    pub fn set_charging(&mut self, enabled: bool) -> OperationResult<E> {
        let mut bits = self.read_u8(POWER_CHARGE1).map_err(Error::I2c)?;
        bits.set_bit(POWER_CHARGE1_ENABLE_CHARGING, enabled);

        self.write_u8(POWER_CHARGE1, bits).map_err(Error::I2c)?;

        Ok(())
    }

    /// Enables or disables certain ADC functions of AXP173.
    pub fn set_adc_settings(&mut self, adc_settings: &AdcSettings) -> OperationResult<E> {
        // Set the contents of ADC enable/disable register.
        let mut bits = self.read_u8(POWER_ADC_EN1).map_err(Error::I2c)?;
        adc_settings.write_adc_en_bits(&mut bits);
        self.write_u8(POWER_ADC_EN1, bits).map_err(Error::I2c)?;

        // Set ADC sample rate
        let mut bits = self.read_u8(POWER_ADC_SPEED_TS).map_err(Error::I2c)?;
        adc_settings.write_adc_sample_rate_and_ts_bits(&mut bits);
        self.write_u8(POWER_ADC_SPEED_TS, bits)
            .map_err(Error::I2c)?;

        Ok(())
    }

    /// Returns VBUS voltage.
    pub fn vbus_voltage(&mut self) -> Axp173Result<Voltage, E> {
        let res = self.read_u12(POWER_VBUS_VOL_H8).map_err(Error::I2c)?;

        Ok(Voltage::new(res, VBUS_VOLTAGE_COEFF))
    }

    /// Returns battery charging current.
    pub fn vbus_current(&mut self) -> Axp173Result<Current, E> {
        let res = self.read_u13(POWER_VBUS_CUR_H8).map_err(Error::I2c)?;

        Ok(Current::new(res, VBUS_CURRENT_COEFF, VBUS_CURRENT_DIV))
    }

    /// Returns battery voltage.
    pub fn batt_voltage(&mut self) -> Axp173Result<Voltage, E> {
        let res = self.read_u12(POWER_BAT_AVERVOL_H8).map_err(Error::I2c)?;

        Ok(Voltage::new(res, BATT_VOLTAGE_COEFF))
    }

    /// Returns battery charging current.
    pub fn batt_charge_current(&mut self) -> Axp173Result<Current, E> {
        let res = self.read_u13(POWER_BAT_AVERCHGCUR_H8).map_err(Error::I2c)?;

        Ok(Current::new(res, BATT_CURRENT_COEFF, BATT_CURRENT_DIV))
    }

    /// Returns battery discharging current.
    pub fn batt_discharge_current(&mut self) -> Axp173Result<Current, E> {
        let res = self
            .read_u13(POWER_BAT_AVERDISCHGCUR_H8)
            .map_err(Error::I2c)?;

        Ok(Current::new(res, BATT_CURRENT_COEFF, BATT_CURRENT_DIV))
    }

    /// Reads a 12-bit integer from two consecutive registers.
    fn read_u12(&mut self, msb_reg: u8) -> Result<u16, E> {
        let msb = self.read_u8(msb_reg)? as u16;
        let lsb = self.read_u8(msb_reg + 1)? as u16 & 0b1111;

        Ok(msb << 4 | lsb)
    }

    /// Reads a 13-bit integer from two consecutive registers.
    fn read_u13(&mut self, msb_reg: u8) -> Result<u16, E> {
        let msb = self.read_u8(msb_reg)? as u16;
        let lsb = self.read_u8(msb_reg + 1)? as u16 & 0b11111;

        Ok(msb << 5 | lsb)
    }

    fn read_u8(&mut self, reg: u8) -> Result<u8, E> {
        let mut byte: [u8; 1] = [0; 1];

        match self.i2c.write_read(AXP173_ADDR, &[reg], &mut byte) {
            Ok(_) => Ok(byte[0]),
            Err(e) => Err(e),
        }
    }

    fn read_bytes(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), E> {
        self.i2c.write_read(AXP173_ADDR, &[reg], buf)
    }

    fn write_u8(&mut self, reg: u8, value: u8) -> Result<(), E> {
        self.i2c.write(AXP173_ADDR, &[reg, value])?;

        Ok(())
    }
}
