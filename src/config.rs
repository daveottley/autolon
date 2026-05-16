use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

pub const APP_ID: &str = "io.github.autolon.Autolon";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub hotkeys: Hotkeys,
    pub clicker: ClickerConfig,
    #[serde(default)]
    pub global_autoclicker_enabled: bool,
    pub backend: BackendPreference,
    pub slots: Slots,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hotkeys {
    pub cycle: String,
    pub stop: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickerConfig {
    pub cycle_order: Vec<u8>,
    pub min_interval_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendPreference {
    #[default]
    Auto,
    Uinput,
    X11,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slots {
    #[serde(rename = "1")]
    #[serde(default = "default_slow_slot")]
    pub one: SlotConfig,
    #[serde(rename = "2")]
    #[serde(default = "default_fast_slot")]
    pub two: SlotConfig,
    #[serde(rename = "3")]
    #[serde(default = "default_user_slot")]
    pub three: SlotConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotConfig {
    pub name: String,
    pub enabled: bool,
    pub button: MouseButton,
    pub interval_ms: u64,
    pub press_duration_ms: u64,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl Default for SlotConfig {
    fn default() -> Self {
        default_user_slot()
    }
}

impl Config {
    pub fn load_or_create() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }

        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self =
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
        if config.hotkeys.stop.eq_ignore_ascii_case("Shift+F6") {
            config.hotkeys.stop = "F7".to_string();
            config.save()?;
        }
        config.normalize_v1()?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported config version {}; expected 1", self.version);
        }
        if self.clicker.min_interval_ms < 1 {
            bail!("clicker.min_interval_ms must be at least 1");
        }
        for slot_id in [1_u8, 2, 3] {
            let slot = self.slot(slot_id)?;
            if slot.interval_ms < self.clicker.min_interval_ms {
                bail!(
                    "slot {} interval {}ms is below minimum {}ms",
                    slot_id,
                    slot.interval_ms,
                    self.clicker.min_interval_ms
                );
            }
            if slot.press_duration_ms == 0 {
                bail!("slot {} press_duration_ms must be at least 1", slot_id);
            }
            if slot.press_duration_ms >= slot.interval_ms {
                bail!(
                    "slot {} press_duration_ms must be less than interval_ms",
                    slot_id
                );
            }
        }
        Ok(())
    }

    pub fn slot(&self, slot_id: u8) -> Result<&SlotConfig> {
        match slot_id {
            1 => Ok(&self.slots.one),
            2 => Ok(&self.slots.two),
            3 => Ok(&self.slots.three),
            _ => bail!("slot id must be 1, 2, or 3"),
        }
    }

    pub fn slot_mut(&mut self, slot_id: u8) -> Result<&mut SlotConfig> {
        match slot_id {
            1 => Ok(&mut self.slots.one),
            2 => Ok(&mut self.slots.two),
            3 => Ok(&mut self.slots.three),
            _ => bail!("slot id must be 1, 2, or 3"),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            hotkeys: Hotkeys {
                cycle: "F6".to_string(),
                stop: "F7".to_string(),
            },
            clicker: ClickerConfig {
                cycle_order: vec![1, 2, 3],
                min_interval_ms: 2,
            },
            global_autoclicker_enabled: true,
            backend: BackendPreference::Auto,
            slots: Slots {
                one: default_slow_slot(),
                two: default_fast_slot(),
                three: default_user_slot(),
            },
        }
    }
}

impl Config {
    fn normalize_v1(&mut self) -> Result<()> {
        if self.clicker.cycle_order != [1, 2, 3] {
            self.clicker.cycle_order = vec![1, 2, 3];
        }
        if self.clicker.min_interval_ms != 2 {
            self.clicker.min_interval_ms = 2;
        }

        let old_two_slot_defaults = self.slots.one.name == "Fast" && self.slots.two.name == "Slow";
        let missing_new_names = self.slots.one.name != "Slow"
            || self.slots.two.name != "Fast"
            || self.slots.three.name != "User";
        if old_two_slot_defaults {
            std::mem::swap(&mut self.slots.one, &mut self.slots.two);
            self.save()?;
        } else if missing_new_names {
            self.slots.one = default_slow_slot();
            self.slots.two = default_fast_slot();
            self.slots.three = default_user_slot();
            self.save()?;
        }
        Ok(())
    }
}

fn default_fast_slot() -> SlotConfig {
    SlotConfig {
        name: "Fast".to_string(),
        enabled: true,
        button: MouseButton::Left,
        interval_ms: 10,
        press_duration_ms: 1,
    }
}

fn default_slow_slot() -> SlotConfig {
    SlotConfig {
        name: "Slow".to_string(),
        enabled: true,
        button: MouseButton::Left,
        interval_ms: 500,
        press_duration_ms: 25,
    }
}

fn default_user_slot() -> SlotConfig {
    SlotConfig {
        name: "User".to_string(),
        enabled: false,
        button: MouseButton::Left,
        interval_ms: 1000,
        press_duration_ms: 25,
    }
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine XDG config directory")?;
    Ok(base.join("autolon").join("config.toml"))
}
