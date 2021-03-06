use alert::*;
use animation::*;
use best::*;
use change::ChangeContext;
use common_animations;
use direction::*;
use entity_store::*;
use event::*;
use goal::*;
use grid_2d;
use grid_2d::Size;
use grid_2d::*;
use input::*;
use message_queues::*;
use meter::*;
use npc_info::*;
use pathfinding::*;
use policy;
use prototypes;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use shadowcast::{self, ShadowcastContext};
use std::collections::HashSet;
use std::iter::Enumerate;
use std::slice;
use std::time::Duration;
use terrain::*;
use tile_info::*;
use transform::*;
use weapons;
use world::World;

const NUM_LEVELS: usize = 6;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
struct VisibilityCell {
    tiles: Vec<TileInfo>,
    last_updated: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VisibilityGrid(Grid<VisibilityCell>);

pub struct VisibilityIter<'a> {
    iter: grid_2d::GridEnumerate<'a, VisibilityCell>,
    time: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Visible,
    Remembered,
}

impl<'a> Iterator for VisibilityIter<'a> {
    type Item = (slice::Iter<'a, TileInfo>, Coord, Visibility);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((coord, cell)) = self.iter.next() {
            let visibility = if cell.last_updated == 0 {
                continue;
            } else if cell.last_updated == self.time {
                Visibility::Visible
            } else {
                Visibility::Remembered
            };
            return Some((cell.tiles.iter(), coord, visibility));
        }
        None
    }
}

impl VisibilityGrid {
    fn new(size: Size) -> Self {
        VisibilityGrid(Grid::new_default(size))
    }
    fn iter(&self, time: u64) -> VisibilityIter {
        VisibilityIter {
            iter: self.0.enumerate(),
            time,
        }
    }
    fn clear(&mut self) {
        for cell in self.0.iter_mut() {
            cell.last_updated = 0;
            cell.tiles.clear();
        }
    }
    fn get(&self, coord: Coord) -> Option<&VisibilityCell> {
        self.0.get(coord)
    }
}

struct VisibilityRefs<'a> {
    grid: &'a mut VisibilityGrid,
    world: &'a World,
}

fn for_each_visible_cell(
    player_coord: Coord,
    time: u64,
    refs: &mut VisibilityRefs,
    ctx: &mut ShadowcastContext<u8>,
) {
    ctx.for_each(
        player_coord,
        &refs.world.spatial_hash,
        shadowcast::vision_distance::Square::new(128),
        1,
        |coord, _, _| {
            if let Some(cell) = refs.grid.0.get_mut(coord) {
                if let Some(sh_cell) = refs.world.spatial_hash.get(coord) {
                    if sh_cell.last_updated > cell.last_updated {
                        cell.tiles.clear();
                        for id in sh_cell.tile_set.iter() {
                            if let Some(&tile_info) = refs.world.entity_store.tile_info.get(&id) {
                                cell.tiles.push(tile_info);
                            }
                        }
                    }
                    cell.last_updated = time;
                }
            }
        },
    );
}

impl shadowcast::InputGrid for SpatialHashTable {
    type Opacity = u8;
    fn size(&self) -> Size {
        SpatialHashTable::size(self)
    }
    fn get_opacity(&self, coord: Coord) -> Self::Opacity {
        self.get(coord)
            .map(|cell| cell.opacity_total)
            .expect("tried to get opacity out of bounds")
    }
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
enum TurnState {
    Player,
    Npcs,
    FastNpcs,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
enum PlayerTurnEvent {
    ChangeActiveMeter(ActiveMeterType, i32),
    ChangePassiveMeter(PassiveMeterType, i32),
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
struct PlayerTurnEventEntry {
    event: PlayerTurnEvent,
    remaining: u32,
    reset: u32,
}

impl PlayerTurnEventEntry {
    fn full(event: PlayerTurnEvent, reset: u32) -> Self {
        Self {
            event,
            remaining: reset,
            reset,
        }
    }
}

pub struct ActiveMeterInfoIter<'a> {
    entity_store: &'a EntityStore,
    entity_id: EntityId,
    meter_metadata: Enumerate<slice::Iter<'a, ActiveMeterType>>,
    selected_meter: Option<ActiveMeterType>,
}

impl<'a> Iterator for ActiveMeterInfoIter<'a> {
    type Item = ActiveMeterInfo;
    fn next(&mut self) -> Option<Self::Item> {
        self.meter_metadata.next().map(|(index, &typ)| {
            let general_typ: MeterType = typ.into();
            let meter = Meter::from_entity_store(self.entity_id, self.entity_store, general_typ)
                .expect("Meter identifiers out of sync with game state");
            ActiveMeterInfo {
                typ,
                identifier: ActiveMeterIdentifier::from_index(index),
                meter,
                is_selected: Some(typ) == self.selected_meter,
            }
        })
    }
}

pub struct PassiveMeterInfoIter<'a> {
    entity_store: &'a EntityStore,
    entity_id: EntityId,
    meter_metadata: slice::Iter<'a, PassiveMeterType>,
    compass_meter: Meter,
}

impl<'a> Iterator for PassiveMeterInfoIter<'a> {
    type Item = PassiveMeterInfo;
    fn next(&mut self) -> Option<Self::Item> {
        self.meter_metadata.next().map(|&typ| {
            let general_typ: MeterType = typ.into();
            let meter = if general_typ == MeterType::Compass {
                self.compass_meter
            } else {
                Meter::from_entity_store(self.entity_id, self.entity_store, general_typ)
                    .expect("Meter list out of sync with game state")
            };
            PassiveMeterInfo { typ, meter }
        })
    }
}

#[derive(Clone, Debug)]
pub struct State {
    world: World,
    messages: MessageQueues,
    swap_messages: MessageQueuesSwap,
    npc_order: Vec<EntityId>,
    seen_animation_channels: HashSet<AnimationChannel>,
    rng: StdRng,
    player_id: EntityId,
    turn: TurnState,
    pathfinding: PathfindingContext,
    change_context: ChangeContext,
    active_meters: Vec<ActiveMeterType>,
    passive_meters: Vec<PassiveMeterType>,
    selected_meter: Option<ActiveMeterType>,
    levels: Vec<TerrainInfo>,
    level_index: usize,
    player_turn_events: Vec<PlayerTurnEventEntry>,
    shadowcast: ShadowcastContext<u8>,
    visibility_grid: VisibilityGrid,
    rng_seed: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveState {
    world: World,
    player_id: EntityId,
    next_rng_seed: usize,
    size: Size,
    turn: TurnState,
    messages: MessageQueues,
    active_meters: Vec<ActiveMeterType>,
    passive_meters: Vec<PassiveMeterType>,
    levels: Vec<TerrainInfo>,
    level_index: usize,
    player_turn_events: Vec<PlayerTurnEventEntry>,
    visibility_grid: VisibilityGrid,
}

fn shuffled_unequipped_meters<R: Rng>(world: &World, id: EntityId, rng: &mut R) -> Vec<MeterType> {
    let mut types = ALL_METER_TYPES
        .iter()
        .cloned()
        .filter(|&typ| {
            let component_type: ComponentType = typ.into();
            let type_set = world.entity_components.get(id);
            if type_set.contains(component_type) {}
            !type_set.contains(component_type)
        })
        .collect::<Vec<_>>();
    types.shuffle(rng);
    types
}

impl State {
    pub fn selected_meter_type(&self) -> Option<ActiveMeterType> {
        self.selected_meter
    }

    pub fn switch_levels_no_upgrade(&mut self) {
        self.switch_levels(None);
    }

    pub fn switch_levels_upgrade(&mut self, upgrade: MeterType) {
        self.switch_levels(Some(upgrade));
    }

    pub fn upgrade_choices(&mut self) -> Vec<MeterType> {
        const NUM_CHOICES: usize = 3;
        let types = shuffled_unequipped_meters(&self.world, self.player_id, &mut self.rng);
        let num_choices = ::std::cmp::min(NUM_CHOICES, types.len());
        types[0..num_choices].iter().cloned().collect()
    }

    fn switch_levels(&mut self, upgrade: Option<MeterType>) {
        self.level_index += 1;
        let mut next_world = World::new(
            &self.levels[self.level_index],
            &mut self.messages,
            &mut self.rng,
        );

        let next_player_id = *next_world
            .entity_store
            .player
            .iter()
            .next()
            .expect("No player");

        for change in self
            .world
            .component_drain_insert(self.player_id, next_player_id)
        {
            if change.typ() == ComponentType::Coord {
                // otherwise the player would be moved to their old position in the new level
                continue;
            }

            next_world.commit(change);
        }

        if let Some(upgrade) = upgrade {
            let component_type: ComponentType = upgrade.into();
            let type_set = next_world.entity_components.get(next_player_id);
            if !type_set.contains(component_type) {
                next_world.commit(EntityChange::Insert(
                    next_player_id,
                    upgrade.player_component_value(),
                ));
                match upgrade.active_or_passive() {
                    ActiveOrPassive::Active(typ) => {
                        self.active_meters.push(typ);
                        let general_typ: MeterType = typ.into();
                        if let Some(change) = general_typ.periodic_change() {
                            let event = PlayerTurnEvent::ChangeActiveMeter(typ, change.change);
                            let entry = PlayerTurnEventEntry::full(event, change.turns);
                            self.player_turn_events.push(entry);
                        }
                    }
                    ActiveOrPassive::Passive(typ) => {
                        self.passive_meters.push(typ);
                        let general_typ: MeterType = typ.into();
                        if let Some(change) = general_typ.periodic_change() {
                            let event = PlayerTurnEvent::ChangePassiveMeter(typ, change.change);
                            let entry = PlayerTurnEventEntry::full(event, change.turns);
                            self.player_turn_events.push(entry);
                        }
                    }
                }
            }
        }

        let player_coord = *next_world
            .entity_store
            .coord
            .get(&next_player_id)
            .expect("No player coord");
        self.messages.player_moved_to = Some(player_coord);

        self.player_id = next_player_id;
        self.world = next_world;
        self.turn = TurnState::Player;

        self.visibility_grid.clear();
        self.update_visibility();

        self.pathfinding
            .update_player_map(player_coord, &self.world.spatial_hash);
    }

    pub fn new(rng_seed: usize) -> Self {
        let mut rng = StdRng::seed_from_u64(rng_seed as u64);

        let mut levels = Vec::new();

        let mut goals = vec![
            GoalType::KillEggs,
            GoalType::KillBoss,
            GoalType::ActivateBeacon,
        ];

        while goals.len() < NUM_LEVELS {
            goals.push(choose_goal_type(&mut rng));
        }

        goals.shuffle(&mut rng);

        for i in 0..(NUM_LEVELS - 1) {
            let config = TerrainConfig {
                final_level: false,
                goal_type: goals.pop().unwrap(),
                level: i as i32,
            };
            let info = TerrainInfo {
                typ: TerrainType::Dungeon,
                config,
            };
            levels.push(info);
        }

        let final_config = TerrainConfig {
            final_level: true,
            goal_type: GoalType::Escape,
            level: NUM_LEVELS as i32 - 1,
        };

        let final_info = TerrainInfo {
            typ: TerrainType::Dungeon,
            config: final_config,
        };

        levels.push(final_info);

        let level_index = 0;

        let mut messages = MessageQueues::new();

        let mut world = World::new(&levels[level_index], &mut messages, &mut rng);

        let player_id = *world.entity_store.player.iter().next().expect("No player");

        let player_coord = *world
            .entity_store
            .coord
            .get(&player_id)
            .expect("No player coord");
        messages.player_moved_to = Some(player_coord);

        let random_meter = shuffled_unequipped_meters(&world, player_id, &mut rng)
            .pop()
            .expect("Couldn't find random meter to initialise player");

        world.commit(EntityChange::Insert(
            player_id,
            random_meter.player_component_value(),
        ));

        let active_meters: Vec<_> = world
            .entity_components
            .component_types(player_id)
            .filter_map(|typ| MeterType::from_component_type(typ).and_then(|typ| typ.active()))
            .collect();

        let passive_meters: Vec<_> = world
            .entity_components
            .component_types(player_id)
            .filter_map(|typ| MeterType::from_component_type(typ).and_then(|typ| typ.passive()))
            .collect();

        let mut player_turn_events = Vec::new();

        for typ in active_meters.iter().cloned() {
            let general_typ: MeterType = typ.into();
            if let Some(change) = general_typ.periodic_change() {
                let event = PlayerTurnEvent::ChangeActiveMeter(typ, change.change);
                let entry = PlayerTurnEventEntry::full(event, change.turns);
                player_turn_events.push(entry);
            }
        }

        for typ in passive_meters.iter().cloned() {
            let general_typ: MeterType = typ.into();
            if let Some(change) = general_typ.periodic_change() {
                let event = PlayerTurnEvent::ChangePassiveMeter(typ, change.change);
                let entry = PlayerTurnEventEntry::full(event, change.turns);
                player_turn_events.push(entry);
            }
        }

        let mut pathfinding = PathfindingContext::new(world.size());

        pathfinding.update_player_map(player_coord, &world.spatial_hash);

        Self {
            player_id,
            rng,
            turn: TurnState::Player,
            messages,
            swap_messages: MessageQueuesSwap::new(),
            pathfinding,
            visibility_grid: VisibilityGrid::new(world.size()),
            npc_order: Vec::new(),
            seen_animation_channels: HashSet::new(),
            change_context: ChangeContext::new(),
            world,
            active_meters,
            passive_meters,
            selected_meter: None,
            levels,
            level_index,
            player_turn_events,
            shadowcast: ShadowcastContext::new(),
            rng_seed,
        }
    }

    pub fn rng_seed(&self) -> usize {
        self.rng_seed
    }

    pub fn save(&self, next_rng_seed: usize) -> SaveState {
        let mut changes = Vec::with_capacity(1024);
        self.world.entity_store.clone_changes(&mut changes);
        SaveState {
            world: self.world.clone(),
            player_id: self.player_id,
            next_rng_seed,
            size: self.world.size(),
            turn: self.turn,
            messages: self.messages.clone(),
            active_meters: self.active_meters.clone(),
            passive_meters: self.passive_meters.clone(),
            levels: self.levels.clone(),
            level_index: self.level_index,
            player_turn_events: self.player_turn_events.clone(),
            visibility_grid: self.visibility_grid.clone(),
        }
    }

    pub fn visible_cells(&self) -> VisibilityIter {
        self.visibility_grid.iter(self.world.count)
    }

    fn update_visibility(&mut self) {
        let &player_coord = self.world.entity_store.coord.get(&self.player_id).unwrap();
        let mut output_grid = VisibilityRefs {
            grid: &mut self.visibility_grid,
            world: &self.world,
        };
        for_each_visible_cell(
            player_coord,
            self.world.count,
            &mut output_grid,
            &mut self.shadowcast,
        );
    }

    pub fn player_active_meter_info(&self) -> ActiveMeterInfoIter {
        ActiveMeterInfoIter {
            entity_store: &self.world.entity_store,
            meter_metadata: self.active_meters.iter().enumerate(),
            entity_id: self.player_id,
            selected_meter: self.selected_meter,
        }
    }

    fn compass_meter(&self) -> Meter {
        const MAX_DISTANCE: i32 = 40;
        let mut closest = BestSet::new();

        if let Some(goal) = self.world.goal_state.as_ref() {
            goal.with_goal_coords(&self.world.entity_store, |coord| {
                if let Some(distance) = self.pathfinding.distance_to_player(coord) {
                    closest.insert_lt(distance as i32);
                }
            });
        }

        if closest.is_empty() {
            if let Some(id) = self.world.entity_store.stairs.iter().next() {
                if let Some(coord) = self.world.entity_store.coord.get(id) {
                    if let Some(distance) = self.pathfinding.distance_to_player(*coord) {
                        closest.insert_lt(distance as i32);
                    }
                }
            }
        }

        Meter::new(closest.into_value().unwrap_or(MAX_DISTANCE), MAX_DISTANCE)
    }

    pub fn player_passive_meter_info(&self) -> PassiveMeterInfoIter {
        PassiveMeterInfoIter {
            entity_store: &self.world.entity_store,
            meter_metadata: self.passive_meters.iter(),
            entity_id: self.player_id,
            compass_meter: self.compass_meter(),
        }
    }

    pub fn entity_store(&self) -> &EntityStore {
        &self.world.entity_store
    }
    pub fn spatial_hash(&self) -> &SpatialHashTable {
        &self.world.spatial_hash
    }

    pub fn goal_info(&self) -> Option<(GoalType, bool)> {
        self.world
            .goal_state
            .as_ref()
            .map(|s| (s.typ(), s.is_complete(&self.world.entity_store)))
    }

    pub fn with_goal_meters<F>(&self, f: F)
    where
        F: FnMut(GoalMeterInfo),
    {
        self.world
            .goal_state
            .as_ref()
            .map(|s| s.with_goal_meters(&self.world.entity_store, f));
    }

    pub fn overall_progress_meter(&self) -> Meter {
        Meter {
            value: (self.levels.len() - self.level_index) as i32 * 10,
            max: self.levels.len() as i32 * 10,
        }
    }

    fn use_rail_gun(&mut self, direction: CardinalDirection) -> Result<(), Alert> {
        let mut ammo = self
            .world
            .entity_store
            .rail_gun_meter
            .get(&self.player_id)
            .cloned()
            .unwrap();

        if ammo.value > 0 {
            let entity_coord = self
                .world
                .entity_store
                .coord
                .get(&self.player_id)
                .cloned()
                .unwrap();

            let start_coord = entity_coord + direction.coord();

            let mut coord = start_coord;
            loop {
                if let Some(cell) = self.world.spatial_hash.get(coord) {
                    if cell.solid_count > 0 {
                        break;
                    }
                } else {
                    break;
                };

                let shot_id = self.world.id_allocator.allocate();
                common_animations::rail_gun_shot(shot_id, coord, direction, &mut self.messages);

                coord += direction.coord();
            }

            ammo.value -= 1;
            self.messages
                .change(insert::rail_gun_meter(self.player_id, ammo));

            Ok(())
        } else {
            Err(Alert::NoAmmo)
        }
    }

    fn use_gun(&mut self) -> Result<(), Alert> {
        let mut ammo = self
            .world
            .entity_store
            .gun_meter
            .get(&self.player_id)
            .cloned()
            .unwrap();

        if ammo.value > 0 {
            let entity_coord = self
                .world
                .entity_store
                .coord
                .get(&self.player_id)
                .cloned()
                .unwrap();

            for direction in CardinalDirections {
                let start_coord = entity_coord + direction.coord();
                let bullet_id = self.world.id_allocator.allocate();
                prototypes::bullet(
                    bullet_id,
                    start_coord,
                    direction,
                    weapons::GUN_BULLET_RANGE,
                    &mut self.messages,
                );

                common_animations::bullet(bullet_id, &mut self.messages);
            }
            ammo.value -= 1;
            self.messages
                .change(insert::gun_meter(self.player_id, ammo));
            Ok(())
        } else {
            Err(Alert::NoAmmo)
        }
    }

    fn use_push(&mut self) -> Result<(), Alert> {
        let mut push = self
            .world
            .entity_store
            .push_meter
            .get(&self.player_id)
            .cloned()
            .unwrap();
        if push.value > 0 {
            push.value -= 1;
            self.messages
                .change(insert::push_meter(self.player_id, push));

            let entity_coord = self
                .world
                .entity_store
                .coord
                .get(&self.player_id)
                .cloned()
                .unwrap();

            for direction in CardinalDirections {
                let start_coord = entity_coord + direction.coord();
                let id = self.world.id_allocator.allocate();
                common_animations::push_wave(
                    id,
                    start_coord,
                    true,
                    true,
                    true,
                    direction,
                    8,
                    &mut self.messages,
                );
            }
            Ok(())
        } else {
            Err(Alert::NoAmmo)
        }
    }
    fn use_metabol(&mut self) -> Result<(), Alert> {
        let mut metabol = self
            .world
            .entity_store
            .metabol_meter
            .get(&self.player_id)
            .cloned()
            .unwrap();
        if metabol.value > 0 {
            metabol.value -= 1;
            self.messages
                .change(insert::metabol_meter(self.player_id, metabol));

            let entity_coord = self
                .world
                .entity_store
                .coord
                .get(&self.player_id)
                .cloned()
                .unwrap();

            for direction in CardinalDirections {
                let start_coord = entity_coord + direction.coord();
                let id = self.world.id_allocator.allocate();
                common_animations::metabol_wave(
                    id,
                    start_coord,
                    true,
                    true,
                    true,
                    direction,
                    8,
                    &mut self.messages,
                );
            }
            Ok(())
        } else {
            Err(Alert::NoAmmo)
        }
    }
    fn use_medkit(&mut self) -> Result<(), Alert> {
        let mut medkit = self
            .world
            .entity_store
            .medkit_meter
            .get(&self.player_id)
            .cloned()
            .unwrap();
        if medkit.value > 0 {
            let heal_amount = medkit.value;
            medkit.value = -1;
            self.messages
                .change(insert::medkit_meter(self.player_id, medkit));

            let mut health = self
                .world
                .entity_store
                .health_meter
                .get(&self.player_id)
                .cloned()
                .unwrap();
            health.value = ::std::cmp::min(health.value + heal_amount, health.max);
            self.messages
                .change(insert::health_meter(self.player_id, health));
            Ok(())
        } else {
            Err(Alert::NoMedkit)
        }
    }

    fn walk(&mut self, direction: CardinalDirection) -> Result<(), Alert> {
        let current = *self.world.entity_store.coord.get(&self.player_id).unwrap();
        let next = current + direction.coord();

        if let Some(sh_cell) = self.world.spatial_hash.get(next) {
            let door_cell = sh_cell.door_count > 0;
            let solid_cell = sh_cell.solid_count > 0 && !door_cell;
            if solid_cell {
                return Err(Alert::WalkIntoWall);
            }
        }

        self.messages
            .changes
            .push(insert::coord(self.player_id, next));

        Ok(())
    }

    fn blink(&mut self, direction: CardinalDirection) -> Result<(), Alert> {
        let mut blink = self
            .world
            .entity_store
            .blink_meter
            .get(&self.player_id)
            .cloned()
            .unwrap();

        if blink.value > 0 {
            blink.value -= 2; // XXX
            self.messages
                .change(insert::blink_meter(self.player_id, blink));

            let current = *self.world.entity_store.coord.get(&self.player_id).unwrap();
            let next = current + direction.coord() + direction.coord();

            if let Some(sh_cell) = self.world.spatial_hash.get(next) {
                let door_cell = sh_cell.door_count > 0;
                let npc_cell = !sh_cell.npc_set.is_empty();
                let solid_cell = sh_cell.solid_count > 0 && !door_cell;
                if solid_cell || npc_cell {
                    return Err(Alert::BlinkIntoNonEmpty);
                }
            }

            self.messages
                .changes
                .push(insert::coord(self.player_id, next));

            Ok(())
        } else {
            Err(Alert::NoBlink)
        }
    }

    fn player_turn(&mut self, input: Input) -> Option<Event> {
        match input {
            Input::Direction(direction) => {
                match self.selected_meter {
                    None => {
                        if let Err(alert) = self.walk(direction) {
                            return Some(Event::External(ExternalEvent::Alert(alert)));
                        }
                    }
                    Some(ActiveMeterType::Gun) => return None,
                    Some(ActiveMeterType::Medkit) => return None,
                    Some(ActiveMeterType::Metabol) => return None,
                    Some(ActiveMeterType::Push) => return None,
                    Some(ActiveMeterType::Blink) => {
                        if let Err(alert) = self.blink(direction) {
                            self.selected_meter = None;
                            return Some(Event::External(ExternalEvent::Alert(alert)));
                        }
                    }
                    Some(ActiveMeterType::RailGun) => {
                        if let Err(alert) = self.use_rail_gun(direction) {
                            self.selected_meter = None;
                            return Some(Event::External(ExternalEvent::Alert(alert)));
                        }
                    }
                }

                self.selected_meter = None;
            }
            Input::ActiveMeterSelect(identifier) => {
                if let Some(meter_type) = self.active_meters.get(identifier.to_index()).cloned() {
                    match meter_type {
                        ActiveMeterType::Gun => {
                            if let Err(alert) = self.use_gun() {
                                return Some(Event::External(ExternalEvent::Alert(alert)));
                            }
                        }
                        ActiveMeterType::Medkit => {
                            if let Err(alert) = self.use_medkit() {
                                return Some(Event::External(ExternalEvent::Alert(alert)));
                            }
                        }
                        ActiveMeterType::Metabol => {
                            if let Err(alert) = self.use_metabol() {
                                return Some(Event::External(ExternalEvent::Alert(alert)));
                            }
                        }
                        ActiveMeterType::Push => {
                            if let Err(alert) = self.use_push() {
                                return Some(Event::External(ExternalEvent::Alert(alert)));
                            }
                        }
                        ActiveMeterType::Blink => {
                            self.selected_meter = Some(meter_type);
                            return Some(Event::External(ExternalEvent::Alert(
                                Alert::BlinkWhichDirection,
                            )));
                        }
                        ActiveMeterType::RailGun => {
                            self.selected_meter = Some(meter_type);
                            return Some(Event::External(ExternalEvent::Alert(
                                Alert::RailgunWhichDirection,
                            )));
                        }
                    }
                } else {
                    return Some(Event::External(ExternalEvent::Alert(Alert::NoSuchMeter)));
                }
            }
            Input::MeterDeselect => {
                self.selected_meter = None;
                return None;
            }
            Input::Wait => (),
        }

        if let Err(alert) = policy::precheck(
            &self.messages.changes,
            &self.world.entity_store,
            &self.world.spatial_hash,
        ) {
            self.messages.changes.clear();
            if let Some(alert) = alert {
                return Some(Event::External(ExternalEvent::Alert(alert)));
            } else {
                return None;
            }
        }

        self.turn = TurnState::Npcs;

        let ret = self.change_context.process(
            &mut self.world,
            &mut self.messages,
            &mut self.swap_messages,
            &mut self.rng,
        );

        self.process_turn_events();

        ret
    }

    fn fast_npc_turns(&mut self) -> Option<Event> {
        self.turn = TurnState::Player;

        self.npc_order.clear();
        for (&id, info) in self.world.entity_store.npc.iter() {
            if !info.fast {
                continue;
            }
            let active = if info.active {
                true
            } else {
                let coord = self.world.entity_store.coord.get(&id).unwrap();
                let visibility = self.visibility_grid.get(*coord).unwrap();
                if visibility.last_updated == self.world.count {
                    self.messages.change(insert::npc(
                        id,
                        NpcInfo {
                            active: true,
                            ..*info
                        },
                    ));
                    true
                } else {
                    false
                }
            };
            if active {
                self.npc_order.push(id);
            }
        }

        self.pathfinding
            .sort_entities_by_distance_to_player(&self.world.entity_store, &mut self.npc_order);

        for &id in self.npc_order.iter() {
            self.pathfinding.act(
                id,
                &self.world.entity_store,
                &self.world.spatial_hash,
                PathfindingConfig { open_doors: true },
                &mut self.messages,
            );
            if let Some(meta) = self.change_context.process(
                &mut self.world,
                &mut self.messages,
                &mut self.swap_messages,
                &mut self.rng,
            ) {
                return Some(meta);
            }
        }

        None
    }

    fn all_npc_turns(&mut self) -> Option<Event> {
        if let Some(player_coord) = self.messages.player_moved_to.take() {
            self.pathfinding
                .update_player_map(player_coord, &self.world.spatial_hash);
        }

        let mut at_least_one_fast = false;
        self.npc_order.clear();
        for (&id, info) in self.world.entity_store.npc.iter() {
            at_least_one_fast = at_least_one_fast || info.fast;
            let active = if info.active {
                true
            } else {
                let coord = self.world.entity_store.coord.get(&id).unwrap();
                let visibility = self.visibility_grid.get(*coord).unwrap();
                if visibility.last_updated == self.world.count {
                    self.messages.change(insert::npc(
                        id,
                        NpcInfo {
                            active: true,
                            ..*info
                        },
                    ));
                    true
                } else {
                    false
                }
            };
            if active && info.mobile {
                self.npc_order.push(id);
            }

            if let Some(&countdown) = self.world.entity_store.countdown.get(&id) {
                if let Some(mut tile_info) = self.world.entity_store.tile_info.get(&id).cloned() {
                    tile_info.countdown = Some(countdown);
                    self.messages.change(insert::tile_info(id, tile_info));
                }
                if countdown == 0 {
                    if let Some(&transform) = self.world.entity_store.transform.get(&id) {
                        if let Some(&coord) = self.world.entity_store.coord.get(&id) {
                            self.messages.change(remove::delayed_transform(id));
                            match transform {
                                Transform::Chrysalis => {
                                    prototypes::chrysalis(
                                        id,
                                        coord,
                                        &mut self.messages,
                                        &mut self.rng,
                                    );
                                }
                                Transform::Queen => {
                                    prototypes::queen(id, coord, false, &mut self.messages);
                                    self.messages.change(remove::countdown(id));
                                }
                                Transform::Aracnoid => {
                                    prototypes::aracnoid(id, coord, &mut self.messages);
                                    self.messages.change(remove::countdown(id));
                                }
                                Transform::Beetoid => {
                                    prototypes::beetoid(id, coord, &mut self.messages);
                                    self.messages.change(remove::countdown(id));
                                }
                                Transform::Larvae => {
                                    prototypes::larvae(
                                        id,
                                        coord,
                                        &mut self.messages,
                                        &mut self.rng,
                                    );
                                }
                            }
                        }
                    }
                } else {
                    self.messages
                        .changes
                        .push(insert::countdown(id, countdown - 1));
                }
            }
        }

        if at_least_one_fast {
            self.turn = TurnState::FastNpcs;
        } else {
            self.turn = TurnState::Player;
        }

        self.pathfinding
            .sort_entities_by_distance_to_player(&self.world.entity_store, &mut self.npc_order);

        let mut event = None;

        for &id in self.npc_order.iter() {
            self.pathfinding.act(
                id,
                &self.world.entity_store,
                &self.world.spatial_hash,
                PathfindingConfig { open_doors: true },
                &mut self.messages,
            );
            if let Some(Event::External(meta)) = self.change_context.process(
                &mut self.world,
                &mut self.messages,
                &mut self.swap_messages,
                &mut self.rng,
            ) {
                match meta {
                    ExternalEvent::Lose | ExternalEvent::Win | ExternalEvent::Ascend(_) => {
                        return Some(Event::External(meta));
                    }
                    ExternalEvent::Alert(_) => event = Some(Event::External(meta)),
                }
            }
        }

        event
    }

    fn animation_tick(&mut self, period: Duration) -> Option<Event> {
        self.seen_animation_channels.clear();

        for animation in swap_drain!(animations, self.messages, self.swap_messages) {
            if let Some(channel) = animation.channel {
                if self.seen_animation_channels.contains(&channel) {
                    self.messages.animations.push(animation);
                    continue;
                }
            }

            match animation.step(period, &self.world.entity_store, &mut self.messages) {
                AnimationStatus::ContinuingOnChannel(channel) => {
                    self.seen_animation_channels.insert(channel);
                }
                AnimationStatus::Finished | AnimationStatus::Continuing => (),
            }
        }
        self.change_context.process(
            &mut self.world,
            &mut self.messages,
            &mut self.swap_messages,
            &mut self.rng,
        )
    }

    fn process_turn_events(&mut self) -> Option<Event> {
        for entry in self.player_turn_events.iter_mut() {
            if entry.remaining == 0 {
                let change = match entry.event {
                    PlayerTurnEvent::ChangeActiveMeter(typ, change) => {
                        let general_typ: MeterType = typ.into();
                        let mut meter = Meter::from_entity_store(
                            self.player_id,
                            &self.world.entity_store,
                            general_typ,
                        )
                        .expect("Missing meter for player turn event");
                        meter.value =
                            ::std::cmp::max(::std::cmp::min(meter.value + change, meter.max), 0);
                        let typ: MeterType = typ.into();
                        typ.insert(self.player_id, meter)
                    }
                    PlayerTurnEvent::ChangePassiveMeter(typ, change) => match typ {
                        PassiveMeterType::Stamina => {
                            let stamina_tick = self
                                .world
                                .entity_store
                                .stamina_tick
                                .get(&self.player_id)
                                .unwrap();
                            insert::stamina_tick(self.player_id, stamina_tick + 1)
                        }
                        _ => {
                            let general_typ: MeterType = typ.into();
                            let mut meter = Meter::from_entity_store(
                                self.player_id,
                                &self.world.entity_store,
                                general_typ,
                            )
                            .expect("Missing meter for player turn event");
                            meter.value = ::std::cmp::max(
                                ::std::cmp::min(meter.value + change, meter.max),
                                0,
                            );
                            let typ: MeterType = typ.into();
                            typ.insert(self.player_id, meter)
                        }
                    },
                };
                self.messages.changes.push(change);
                entry.remaining = entry.reset;
            } else {
                entry.remaining -= 1;
            }
        }

        let ret = self.change_context.process(
            &mut self.world,
            &mut self.messages,
            &mut self.swap_messages,
            &mut self.rng,
        );

        ret
    }

    pub fn tick<I>(&mut self, inputs: I, period: Duration) -> Option<ExternalEvent>
    where
        I: IntoIterator<Item = Input>,
    {
        let event = if self.messages.animations.is_empty() {
            match self.turn {
                TurnState::Player => {
                    if let Some(input) = inputs.into_iter().next() {
                        self.player_turn(input)
                    } else {
                        None
                    }
                }
                TurnState::Npcs => {
                    if let Some(event) = self.all_npc_turns() {
                        Some(event)
                    } else {
                        None
                    }
                }
                TurnState::FastNpcs => self.fast_npc_turns(),
            }
        } else {
            self.animation_tick(period)
        };

        self.update_visibility();

        match event {
            Some(Event::External(external_event)) => Some(external_event),
            None => None,
        }
    }
}

impl From<SaveState> for State {
    fn from(
        SaveState {
            world,
            player_id,
            next_rng_seed,
            size,
            turn,
            messages,
            active_meters,
            passive_meters,
            levels,
            level_index,
            player_turn_events,
            visibility_grid,
        }: SaveState,
    ) -> Self {
        Self {
            world,
            player_id,
            rng: StdRng::seed_from_u64(next_rng_seed as u64),
            turn,
            messages,
            swap_messages: MessageQueuesSwap::new(),
            pathfinding: PathfindingContext::new(size),
            npc_order: Vec::new(),
            seen_animation_channels: HashSet::new(),
            change_context: ChangeContext::new(),
            active_meters,
            passive_meters,
            selected_meter: None,
            levels,
            level_index,
            player_turn_events,
            shadowcast: ShadowcastContext::new(),
            visibility_grid,
            rng_seed: next_rng_seed,
        }
    }
}
