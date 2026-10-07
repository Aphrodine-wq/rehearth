-- Drives a benchmark run: waits for the game to start, pins the game speed high so
-- the sim is CPU-bound, then samples game-time vs real-time and logs the results.
--
-- Every line it writes starts with "BENCH" so tools/bench.sh can grep them out of
-- stonehearth.log. Config (all under mods.rehearth_bench.*):
--   speed            requested game speed (default 1000, far above what the sim can reach)
--   warmup_seconds   real seconds to run before measuring (default 30)
--   duration_seconds real seconds to measure (default 180)
--   sample_ms        sample period in real ms (default 5000)

local BenchController = class()

local function cfg(key, default)
   return radiant.util.get_global_config('mods.rehearth_bench.' .. key, default)
end

local function out(format, ...)
   radiant.log.write('rehearth_bench', 0, 'BENCH ' .. format, ...)
end

local function lua_kb()
   local ok, kb = pcall(collectgarbage, 'count')
   return ok and kb or -1
end

function BenchController:__init(world_name, world)
   self._world = world
   self._speed = cfg('speed', 1000)
   self._warmup_s = cfg('warmup_seconds', 30)
   self._duration_s = cfg('duration_seconds', 180)
   self._sample_ms = cfg('sample_ms', 5000)
   self._label = cfg('label', 'unlabeled')

   self._phase = 'boot'
   self._rates = {}

   out('boot label=%s world=%s speed=%s warmup=%ds duration=%ds',
       self._label, world_name, tostring(self._speed), self._warmup_s, self._duration_s)

   self._interval = radiant.set_realtime_interval('rehearth bench sample', self._sample_ms, function()
         self:_sample()
      end)
end

function BenchController:_ensure_speed()
   local gs = stonehearth.game_speed
   if gs and gs:get_game_speed() ~= self._speed then
      gs:set_game_speed(self._speed, false)
   end
end

function BenchController:_world_stats()
   if self._world.get_stats then
      return self._world:get_stats()
   end
   return ''
end

function BenchController:_sample()
   local game_ms = radiant.gamestate.now()
   local real_s = radiant.get_realtime()

   if self._phase == 'boot' then
      -- the clock only moves once the client has joined and the game is running
      if self._boot_game_ms and game_ms > self._boot_game_ms then
         self:_ensure_speed()
         self._phase = 'warmup'
         self._phase_start_real = real_s
         out('started game_ms=%d', game_ms)
      end
      self._boot_game_ms = game_ms
      self._last_game_ms, self._last_real_s = game_ms, real_s
      return
   end

   self:_ensure_speed()

   local d_game = (game_ms - self._last_game_ms) / 1000
   local d_real = real_s - self._last_real_s
   local rate = d_real > 0 and d_game / d_real or 0
   self._last_game_ms, self._last_real_s = game_ms, real_s

   if self._phase == 'warmup' then
      out('warmup rate=%.2f lua_kb=%d %s', rate, lua_kb(), self:_world_stats())
      if real_s - self._phase_start_real >= self._warmup_s then
         self._phase = 'measure'
         self._phase_start_real = real_s
         self._measure_start_game_ms = game_ms
      end
      return
   end

   if self._phase == 'measure' then
      table.insert(self._rates, rate)
      out('sample t=%.0f rate=%.2f lua_kb=%d %s',
          real_s - self._phase_start_real, rate, lua_kb(), self:_world_stats())
      if real_s - self._phase_start_real >= self._duration_s then
         self:_finish(game_ms, real_s)
      end
   end
end

function BenchController:_finish(game_ms, real_s)
   self._phase = 'done'
   local real = real_s - self._phase_start_real
   local game = (game_ms - self._measure_start_game_ms) / 1000

   local sorted = {}
   for i, r in ipairs(self._rates) do
      sorted[i] = r
   end
   table.sort(sorted)
   local function pct(p)
      if #sorted == 0 then
         return 0
      end
      return sorted[math.max(1, math.floor(#sorted * p + 0.5))]
   end

   out('summary label=%s avg_rate=%.3f p10=%.3f median=%.3f game_s=%.1f real_s=%.1f samples=%d lua_kb=%d %s',
       self._label, real > 0 and game / real or 0, pct(0.1), pct(0.5), game, real, #sorted, lua_kb(),
       self:_world_stats())

   if self._interval then
      self._interval:destroy()
      self._interval = nil
   end
   -- radiant.exit() is ignored outside autotests; tools/bench.sh closes the game
   -- when it sees the summary line.
end

return BenchController
