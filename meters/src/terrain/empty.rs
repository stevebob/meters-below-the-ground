use entity_store::EntityIdAllocator;
use super::*;

pub fn size() -> Size {
    Size::new(29, 29)
}

pub fn populate(
    config: TerrainConfig,
    id_allocator: &mut EntityIdAllocator,
    messages: &mut MessageQueues,
) {
    let strings = vec![
        "#############################",
        "#...........................#",
        "#.@.<.......................#",
        "#...........................#",
        "#.l.........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#...........................#",
        "#############################",
    ].into_iter()
        .map(|s| s.to_string())
        .collect();
    static_strings::populate(&strings, config, id_allocator, messages);
}
