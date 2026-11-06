//! Windows sensors. All unprivileged:
//! * CPU power  : PDH "Energy Meter" counter (RAPL_Package0_PKG), reported in milliwatts.
//! * GPU power  : NVML (nvml.dll from the NVIDIA driver), loaded dynamically.
//! * CPU load   : GetSystemTimes deltas.
//! * Input idle : GetLastInputInfo.
//! * Power/battery: CallNtPowerInformation(SystemBatteryState).

use crate::*;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Power::*;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------- input idle

pub struct WinInput;

impl InputActivitySensor for WinInput {
    fn idle_for(&mut self) -> Result<Duration, SensorError> {
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        // SAFETY: `info` is a valid, correctly sized LASTINPUTINFO.
        let ok = unsafe { GetLastInputInfo(&mut info) };
        if !ok.as_bool() {
            return Err(SensorError::Failed("GetLastInputInfo failed".into()));
        }
        // SAFETY: no arguments.
        let now = unsafe { GetTickCount() };
        Ok(Duration::from_millis(now.wrapping_sub(info.dwTime) as u64))
    }
}

// ---------------------------------------------------------------- CPU load

pub struct WinCpuLoad {
    prev: Option<(u64, u64, u64)>,
}

impl WinCpuLoad {
    pub fn new() -> Self {
        Self { prev: None }
    }
}

impl Default for WinCpuLoad {
    fn default() -> Self {
        Self::new()
    }
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

impl CpuLoadSensor for WinCpuLoad {
    fn read_percent(&mut self) -> Result<f64, SensorError> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: pointers reference valid local FILETIMEs.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }
            .map_err(|e| SensorError::Failed(format!("GetSystemTimes: {e}")))?;
        let cur = (ft(idle), ft(kernel), ft(user));
        let pct = match self.prev {
            Some((pi, pk, pu)) => {
                let d_idle = cur.0.wrapping_sub(pi) as f64;
                let d_total = (cur.1.wrapping_sub(pk) + cur.2.wrapping_sub(pu)) as f64; // kernel includes idle
                if d_total > 0.0 {
                    ((d_total - d_idle) / d_total * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        self.prev = Some(cur);
        Ok(pct)
    }
}

// ---------------------------------------------------------------- CPU power (PDH Energy Meter)

pub struct WinCpuPower {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

// PDH handles are plain integers owned by this struct.
unsafe impl Send for WinCpuPower {}

const ENERGY_PKG: &str = r"\Energy Meter(RAPL_Package0_PKG)\Power";

impl WinCpuPower {
    /// Fails with `Unavailable` when the platform exposes no Energy Meter counter.
    pub fn new() -> Result<Self, SensorError> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: valid out-pointers, null data source = real-time data.
        let st = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) };
        if st != 0 {
            return Err(SensorError::Failed(format!("PdhOpenQuery status {st:#x}")));
        }
        let mut counter = PDH_HCOUNTER::default();
        let path = wide(ENERGY_PKG);
        // SAFETY: `path` is NUL-terminated and outlives the call.
        let st = unsafe { PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter) };
        if st != 0 {
            // SAFETY: query was opened above.
            unsafe { PdhCloseQuery(query) };
            return Err(SensorError::Unavailable(
                "this PC exposes no 'Energy Meter' CPU power counter (needs Intel RAPL / supported AMD via Windows); using a model instead".into(),
            ));
        }
        // Prime: first collect establishes the baseline.
        // SAFETY: query handle is valid.
        unsafe { PdhCollectQueryData(query) };
        Ok(Self { query, counter })
    }
}

impl Drop for WinCpuPower {
    fn drop(&mut self) {
        // SAFETY: handle owned by self.
        unsafe { PdhCloseQuery(self.query) };
    }
}

impl CpuPowerSensor for WinCpuPower {
    fn read_watts(&mut self) -> Result<f64, SensorError> {
        // SAFETY: valid handles; value struct is plain data.
        unsafe {
            let st = PdhCollectQueryData(self.query);
            if st != 0 {
                return Err(SensorError::Failed(format!("PdhCollectQueryData {st:#x}")));
            }
            let mut value = PDH_FMT_COUNTERVALUE::default();
            let st = PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut value);
            if st != 0 {
                return Err(SensorError::Failed(format!("PdhGetFormattedCounterValue {st:#x}")));
            }
            if value.CStatus != 0 {
                return Err(SensorError::Failed(format!("counter status {:#x}", value.CStatus)));
            }
            // Counter unit is milliwatts (16604 on the reference laptop at ~36% load = 16.6 W).
            Ok(value.Anonymous.doubleValue / 1000.0)
        }
    }
}

// ---------------------------------------------------------------- GPU (NVML)

pub struct WinGpu {
    nvml: nvml_wrapper::Nvml,
}

impl WinGpu {
    pub fn new() -> Result<Self, SensorError> {
        let nvml = nvml_wrapper::Nvml::init()
            .map_err(|e| SensorError::Unavailable(format!("NVIDIA NVML not available ({e}); GPU power will be modelled")))?;
        nvml.device_by_index(0).map_err(|e| SensorError::Unavailable(format!("no NVIDIA device: {e}")))?;
        Ok(Self { nvml })
    }
}

impl GpuPowerSensor for WinGpu {
    fn read(&mut self) -> Result<GpuReading, SensorError> {
        let dev = self.nvml.device_by_index(0).map_err(|e| SensorError::Failed(e.to_string()))?;
        let mw = dev.power_usage().map_err(|e| SensorError::Failed(format!("power_usage: {e}")))?;
        let util = dev.utilization_rates().map_err(|e| SensorError::Failed(format!("utilization: {e}")))?;
        Ok(GpuReading { watts: mw as f64 / 1000.0, util_percent: util.gpu as f64 })
    }
}

// ---------------------------------------------------------------- power source / battery

pub struct WinPowerSource;

impl PowerSourceSensor for WinPowerSource {
    fn read(&mut self) -> Result<PowerSourceReading, SensorError> {
        let mut st = SYSTEM_BATTERY_STATE::default();
        // SAFETY: output buffer is a correctly sized SYSTEM_BATTERY_STATE.
        let status = unsafe {
            CallNtPowerInformation(
                SystemBatteryState,
                None,
                0,
                Some(&mut st as *mut _ as *mut core::ffi::c_void),
                std::mem::size_of::<SYSTEM_BATTERY_STATE>() as u32,
            )
        };
        if status.0 != 0 {
            return Err(SensorError::Failed(format!("CallNtPowerInformation NTSTATUS {:#x}", status.0)));
        }
        let has_battery = st.BatteryPresent;
        const UNKNOWN_RATE: u32 = 0x8000_0000;
        let percent = if has_battery && st.MaxCapacity > 0 {
            Some(((st.RemainingCapacity as u64 * 100) / st.MaxCapacity as u64).min(100) as u8)
        } else {
            None
        };
        Ok(PowerSourceReading {
            has_battery,
            ac_online: Some(st.AcOnLine),
            charging: st.Charging,
            battery_percent: percent,
            battery_rate_mw: if has_battery && st.Rate != UNKNOWN_RATE { Some(st.Rate as i32) } else { None },
        })
    }
}

// ---------------------------------------------------------------- detection

impl SensorHub {
    /// Probe this machine and enable every sensor that works; record why the others do not.
    pub fn detect() -> SensorHub {
        let mut hub = SensorHub {
            input: Some(Box::new(WinInput)),
            cpu_load: Some(Box::new(WinCpuLoad::new())),
            power_source: Some(Box::new(WinPowerSource)),
            ..SensorHub::default()
        };
        match WinCpuPower::new() {
            Ok(s) => hub.cpu_power = Some(Box::new(s)),
            Err(e) => hub.missing.push(SensorIssue { sensor: "cpu_power", message: e.to_string() }),
        }
        match WinGpu::new() {
            Ok(s) => hub.gpu = Some(Box::new(s)),
            Err(e) => hub.missing.push(SensorIssue { sensor: "gpu", message: e.to_string() }),
        }
        hub
    }
}

