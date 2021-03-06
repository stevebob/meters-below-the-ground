use meters::goal::*;
use meters::meter::*;
use prototty::*;
use std::fmt::Write;

fn meter_text_info(typ: MeterType) -> TextInfo {
    let colour = match typ {
        MeterType::Gun => Rgb24::new(150, 200, 50),
        MeterType::Medkit => colours::GREEN,
        MeterType::Metabol => Rgb24::new(127, 0, 0),
        MeterType::Health => colours::BRIGHT_RED,
        MeterType::Stamina => colours::BRIGHT_BLUE,
        MeterType::Kevlar => Rgb24::new(255, 127, 0),
        MeterType::RailGun => Rgb24::new(0, 255, 255),
        MeterType::Blink => Rgb24::new(127, 63, 255),
        MeterType::Compass => Rgb24::new(255, 63, 255),
        MeterType::Push => Rgb24::new(255, 255, 0),
    };
    TextInfo {
        foreground_colour: Some(colour),
        ..Default::default()
    }
}

fn goal_meter_text_info(typ: GoalMeterType) -> TextInfo {
    let colour = match typ {
        GoalMeterType::BossHealth => Rgb24::new(255, 127, 255),
        GoalMeterType::DistanceToExit => colours::WHITE,
        GoalMeterType::SuperEggHealth => Rgb24::new(255, 85, 255),
    };
    TextInfo {
        foreground_colour: Some(colour),
        ..Default::default()
    }
}

pub struct MeterView {
    name_padding: usize,
    meter_width: usize,
    scratch: String,
}

pub fn meter_name(typ: MeterType) -> &'static str {
    match typ {
        MeterType::Gun => "Quadgun - shoot in 4 directions at once",
        MeterType::Medkit => "Medkit - use to heal, recharges over time",
        MeterType::Metabol => "Metabol - delay transformations of nearby enemies",
        MeterType::Push => "Push - push nearby enemies away from you",
        MeterType::Health => "Health - be alive",
        MeterType::Stamina => "Stamina - spend to punch, recharges over time",
        MeterType::Kevlar => "Armour - chance to negate damage",
        MeterType::RailGun => "Railgun - shoot enemies in line",
        MeterType::Blink => "Blink - teleport a short distance",
        MeterType::Compass => "Compass - know the distance to the nearest goal",
    }
}

impl MeterView {
    pub fn new(name_padding: usize, meter_width: usize) -> Self {
        Self {
            name_padding,
            meter_width,
            scratch: String::new(),
        }
    }
    fn write_active_name(&mut self, typ: ActiveMeterType, identifier: char, is_selected: bool) {
        let seperator = if is_selected { "*" } else { " " };

        write!(self.scratch, "{}){}", identifier, seperator).unwrap();
        match typ {
            ActiveMeterType::Gun => {
                write!(self.scratch, "{:1$}", "Quadgun", self.name_padding).unwrap()
            }
            ActiveMeterType::Medkit => {
                write!(self.scratch, "{:1$}", "Medkit", self.name_padding).unwrap()
            }
            ActiveMeterType::Metabol => {
                write!(self.scratch, "{:1$}", "Metabol", self.name_padding).unwrap()
            }
            ActiveMeterType::Push => {
                write!(self.scratch, "{:1$}", "Push", self.name_padding).unwrap()
            }
            ActiveMeterType::RailGun => {
                write!(self.scratch, "{:1$}", "Railgun", self.name_padding).unwrap()
            }
            ActiveMeterType::Blink => {
                write!(self.scratch, "{:1$}", "Blink", self.name_padding).unwrap()
            }
        }
    }
    fn write_passive_name(&mut self, typ: PassiveMeterType) {
        write!(self.scratch, "   ").unwrap();
        match typ {
            PassiveMeterType::Health => {
                write!(self.scratch, "{:1$}", "Health", self.name_padding).unwrap()
            }
            PassiveMeterType::Stamina => {
                write!(self.scratch, "{:1$}", "Stamina", self.name_padding).unwrap()
            }
            PassiveMeterType::Kevlar => {
                write!(self.scratch, "{:1$}", "Armour", self.name_padding).unwrap()
            }
            PassiveMeterType::Compass => {
                write!(self.scratch, "{:1$}", "Compass", self.name_padding).unwrap()
            }
        }
    }
    fn write_goal_name(&mut self, typ: GoalMeterType) {
        write!(self.scratch, "   ").unwrap();
        match typ {
            GoalMeterType::BossHealth => {
                write!(self.scratch, "{:1$}", "Boss", self.name_padding).unwrap()
            }
            GoalMeterType::DistanceToExit => {
                write!(self.scratch, "{:1$}", "Metres", self.name_padding).unwrap()
            }
            GoalMeterType::SuperEggHealth => {
                write!(self.scratch, "{:1$}", "Hatching", self.name_padding).unwrap()
            }
        }
    }
    fn write_meter(&mut self, meter: Meter) {
        let value = ::std::cmp::max(meter.value, 0) as usize;
        let max = ::std::cmp::max(meter.max, 0) as usize;
        let filled_meter_width = (self.meter_width * value) / max;
        let filled_meter_width = if value > 0 && filled_meter_width == 0 {
            1
        } else {
            filled_meter_width
        };
        let remaining_meter_width = self.meter_width.saturating_sub(filled_meter_width);
        for _ in 0..filled_meter_width {
            self.scratch.push('█');
        }
        for _ in 0..remaining_meter_width {
            self.scratch.push('░')
        }

        write!(self.scratch, " {}/{}", value, max).unwrap();
    }
}

impl View<ActiveMeterInfo> for MeterView {
    fn view<G: ViewGrid>(
        &mut self,
        info: &ActiveMeterInfo,
        offset: Coord,
        depth: i32,
        grid: &mut G,
    ) {
        self.scratch.clear();
        self.write_active_name(info.typ, info.identifier.to_char(), info.is_selected);
        self.write_meter(info.meter);
        let info = meter_text_info(info.typ.typ());
        TextInfoStringView.view(&(info, &self.scratch), offset, depth, grid);
    }
}

impl View<PassiveMeterInfo> for MeterView {
    fn view<G: ViewGrid>(
        &mut self,
        info: &PassiveMeterInfo,
        offset: Coord,
        depth: i32,
        grid: &mut G,
    ) {
        self.scratch.clear();
        self.write_passive_name(info.typ);
        self.write_meter(info.meter);
        let info = meter_text_info(info.typ.typ());
        TextInfoStringView.view(&(info, &self.scratch), offset, depth, grid);
    }
}

impl View<GoalMeterInfo> for MeterView {
    fn view<G: ViewGrid>(&mut self, info: &GoalMeterInfo, offset: Coord, depth: i32, grid: &mut G) {
        self.scratch.clear();
        self.write_goal_name(info.typ);
        self.write_meter(info.meter);
        let info = goal_meter_text_info(info.typ);
        TextInfoStringView.view(&(info, &self.scratch), offset, depth, grid);
    }
}

impl View<(&'static str, Meter)> for MeterView {
    fn view<G: ViewGrid>(
        &mut self,
        &(title, meter): &(&'static str, Meter),
        offset: Coord,
        depth: i32,
        grid: &mut G,
    ) {
        self.scratch.clear();
        write!(self.scratch, "{} ", title).unwrap();
        self.write_meter(meter);
        StringView.view(&self.scratch, offset, depth, grid);
    }
}
