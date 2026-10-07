-- A deterministic "big town" stress world: lots of hearthlings, a grid of filtered
-- chests, beds, food, and a large field of loose resources that need hauling.
-- More resources drop in every game hour so the hauling load never dries up.
--
-- Config (under mods.rehearth_bench.*):
--   size, workers, crafters, chests, items, spawn_per_hour, max_items, seed

local MicroWorld = require 'micro_world'
local Point3 = _radiant.csg.Point3

local BigTown = class(MicroWorld)

local function cfg(key, default)
   return radiant.util.get_global_config('mods.rehearth_bench.' .. key, default)
end

local RESOURCES = {
   'stonehearth:resources:wood:oak_log',
   'stonehearth:resources:stone:hunk_of_stone',
   'stonehearth:resources:fiber:silkweed_bundle',
   'stonehearth:resources:copper:ore',
   'stonehearth:resources:coal:lump_of_coal',
}

local FOOD = {
   'stonehearth:food:berries:berry_basket',
   'stonehearth:food:corn:corn_basket',
   'stonehearth:food:pumpkin:pumpkin_basket',
}

-- chest filters cycle through these; nil leaves the chest at its default filter
local FILTERS = {
   { 'stockpile_wood' },
   { 'stockpile_stone' },
   { 'stockpile_ore' },
   { 'stockpile_plant' },
   { 'stockpile_wood', 'stockpile_stone' },
   nil,
}

local CRAFTERS = { 'carpenter', 'mason', 'weaver', 'blacksmith', 'potter', 'cook' }

-- small LCG so the layout is identical every run without touching math.random
local function make_rng(seed)
   local state = seed % 2147483647
   if state <= 0 then
      state = state + 2147483646
   end
   return function(lo, hi)
      state = (state * 48271) % 2147483647
      return lo + state % (hi - lo + 1)
   end
end

function BigTown:__init()
   local size = cfg('size', 192)
   self[MicroWorld]:__init(size)
   self:create_world()

   self._rng = make_rng(cfg('seed', 1234))
   self._half = size / 2 - 4
   self._items = {}
   self._player_id = self:get_session().player_id

   local workers = cfg('workers', 40)
   local crafters = cfg('crafters', 6)
   local chests = cfg('chests', 60)
   local items = cfg('items', 1500)

   local roster = { worker = workers }
   for i = 1, crafters do
      local job = CRAFTERS[(i - 1) % #CRAFTERS + 1]
      roster[job] = (roster[job] or 0) + 1
   end
   local citizens = self:create_settlement(roster, 0, 0, 2)

   self:_place_chests(chests)
   self:_place_beds(#citizens)
   self:_place_food(4)

   for i = 1, items do
      self:_drop_resource()
   end

   local per_hour = cfg('spawn_per_hour', 4)
   local max_items = cfg('max_items', 2500)
   if per_hour > 0 then
      self._spawn_interval = stonehearth.calendar:set_interval('rehearth bench resource drop', '1h', function()
            for i = 1, per_hour do
               if #self._items >= max_items then
                  break
               end
               self:_drop_resource()
            end
         end)
   end
end

-- chests in a block east of the banner
function BigTown:_place_chests(count)
   local cols = 10
   for i = 0, count - 1 do
      local x = 16 + (i % cols) * 2
      local z = -10 + math.floor(i / cols) * 2
      local chest = self:place_item('stonehearth:containers:stone_chest', x, z, self._player_id, { force_iconic = false })
      local filter = FILTERS[i % #FILTERS + 1]
      if filter then
         chest:get_component('stonehearth:storage'):set_filter(filter)
      end
   end
end

-- beds in a block west of the banner
function BigTown:_place_beds(count)
   local cols = 10
   for i = 0, count - 1 do
      local x = -30 + (i % cols) * 3
      local z = -10 + math.floor(i / cols) * 4
      self:place_item('stonehearth:furniture:comfy_bed', x, z, self._player_id, { force_iconic = false })
   end
end

function BigTown:_place_food(chests)
   for i = 0, chests - 1 do
      local chest = self:place_item('stonehearth:containers:stone_chest', -6 + i * 2, 14, self._player_id, { force_iconic = false })
      self:fill_storage(chest, FOOD[i % #FOOD + 1])
   end
end

-- loose resources anywhere outside the town core
function BigTown:_drop_resource()
   local rng, half = self._rng, self._half
   local x, z
   repeat
      x, z = rng(-half, half), rng(-half, half)
   until math.abs(x) > 40 or math.abs(z) > 24
   local uri = RESOURCES[rng(1, #RESOURCES)]
   local item = self:place_item(uri, x, z, self._player_id)
   table.insert(self._items, item)
end

-- "items=<total> loose=<still in the world>" for the bench log
function BigTown:get_stats()
   local loose = 0
   local kept = {}
   for _, item in ipairs(self._items) do
      if item:is_valid() then
         table.insert(kept, item)
         if radiant.entities.get_world_location(item) then
            loose = loose + 1
         end
      end
   end
   self._items = kept
   return string.format('items=%d loose=%d', #kept, loose)
end

return BigTown
