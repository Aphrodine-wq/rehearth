-- ReHearth benchmark entry point.
-- Run with: --game.main_mod=rehearth_bench --mods.directory.rehearth_bench.enabled=true
--
-- The 1.1 engine only fires 'radiant:new_game' after the title screen asks for a new
-- game, which never happens under a main_mod with skip_title (microworld hangs on a
-- black screen the same way). So we build the world ourselves as soon as the local
-- player connects, then start the sim and hand off to the bench controller.

rehearth_bench = {}

radiant.log.write('rehearth_bench', 0, 'BENCH loaded')

local started = false

local function start_bench()
   if started then
      return
   end
   started = true

   local world_name = radiant.util.get_global_config('mods.rehearth_bench.world', 'big_town')
   radiant.log.write('rehearth_bench', 0, 'BENCH building world=%s', world_name)

   local World = require(string.format('worlds.%s_world', world_name))
   if not World then
      error(string.format('rehearth_bench: no world script "%s"', world_name))
   end

   local world = World()
   local Controller = require 'bench_controller'
   rehearth_bench.controller = Controller(world_name, world)

   _radiant.sim.start_game()
end

radiant.events.listen(rehearth_bench, 'radiant:new_game', start_bench)
radiant.events.listen(radiant, 'radiant:client_joined', start_bench)

return rehearth_bench
