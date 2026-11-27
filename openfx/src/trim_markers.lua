-- Puts Resolve markers on the trim points of a Gyroflow project: on the clips of the project's video on the active
-- timeline, or on the timeline's ruler where those clips show the trim points. Run through fuscript by the plugin
-- (see trim_markers.rs), which defines REQUEST above this text:
--   video       the project's video file
--   num_frames  its frame count, nil when unknown
--   ranges      the trim ranges in source frames: { { trim_in, trim_out }, ... }
--   color       a Resolve marker color
--   target      "clip" or "timeline"
--   dry_run     only say what would be done
-- The markers made here are told by their custom data, so running it again for the same video replaces them, and the
-- user's own markers are never touched.

local NAME_IN, NAME_OUT = "GF trim in", "GF trim out"

local function log(fmt, ...)
    print(string.format(fmt, ...))
end

-- Calls f and returns its first result, or nil if it raised an error
local function try(f)
    local ok, r = pcall(f)
    if ok then return r end
    return nil
end

local function round(x)
    return math.floor(x + 0.5)
end

local function int(x)
    return string.format("%d", round(x))
end

-- Accepts 59.94, "59.94", "59,94", "59.940 fps"
local function num(v)
    if type(v) == "number" then return v end
    if type(v) == "string" then
        local s = v:gsub(",", ".")
        s = s:match("%-?%d+%.?%d*")
        if s then return tonumber(s) end
    end
    return nil
end

-- For comparing two paths: forward slashes, no doubled slashes, lower case
local function norm_path(p)
    local s = p:gsub("\\", "/")
    local unc = s:match("^//") ~= nil
    s = s:gsub("/+", "/")
    if unc then s = "/" .. s end
    return s:lower()
end

local function base_name(p)
    local s = p:gsub("\\", "/")
    return s:match("([^/]+)$") or s
end

local function get_resolve()
    local r = rawget(_G, "resolve")
    if r then return r end
    if type(rawget(_G, "Resolve")) == "function" then
        r = try(function() return Resolve() end)
        if r then return r end
    end
    local b = rawget(_G, "bmd")
    if b and b.scriptapp then
        return try(function() return b.scriptapp("Resolve") end)
    end
    return nil
end

-- Turns an API list into a plain array (API lists are 1-based arrays, this also copes with one that is keyed differently)
local function as_array(list)
    local out = {}
    if type(list) ~= "table" then return out end
    if #list > 0 then
        for i = 1, #list do out[i] = list[i] end
    else
        for _, v in pairs(list) do out[#out + 1] = v end
    end
    return out
end

-- Every video clip on the timeline that comes from a media file
local function collect_items(timeline)
    local items = {}
    local tracks = num(try(function() return timeline:GetTrackCount("video") end)) or 0
    for t = 1, tracks do
        local list = as_array(try(function() return timeline:GetItemListInTrack("video", t) end))
        for _, item in ipairs(list) do
            local mpi = try(function() return item:GetMediaPoolItem() end)
            if mpi then
                local fp = try(function() return mpi:GetClipProperty("File Path") end)
                if type(fp) == "string" and fp ~= "" then
                    items[#items + 1] = { item = item, mpi = mpi, npath = norm_path(fp), track = t }
                end
            end
        end
    end
    return items
end

-- The clips of the project's video: the ones of that file, or else, as the video may have been moved since the project
-- was saved, the ones of a file of that name with as many frames
local function find_clips(items)
    local wanted = norm_path(REQUEST.video)
    local matches = {}
    for _, e in ipairs(items) do
        if e.npath == wanted then matches[#matches + 1] = e end
    end
    if #matches > 0 then return matches, false end
    local name = base_name(wanted)
    for _, e in ipairs(items) do
        if base_name(e.npath) == name then
            local frames = num(try(function() return e.mpi:GetClipProperty("Frames") end))
            if frames == nil or REQUEST.num_frames == nil or frames == REQUEST.num_frames then
                matches[#matches + 1] = e
            end
        end
    end
    return matches, true
end

-- Works out which of its frames on the timeline a clip shows its source frames at. Returns
--   { start     the clip's first frame on the timeline
--     dur       how many timeline frames it lasts
--     left      how many timeline frames of the video come before its first one
--     offset    function(source frame) -> the frame of the clip showing it, counted from `start` (outside 0..dur-1 when
--               the clip doesn't show it) }
-- and a list of notes, or nil and the reason
local function make_mapper(entry, tl_fps)
    local item, mpi = entry.item, entry.mpi
    local start = num(try(function() return item:GetStart() end))
    local finish = num(try(function() return item:GetEnd() end))
    local dur = num(try(function() return item:GetDuration() end))
    if not start then return nil, "Resolve did not report where the clip starts" end
    if not dur and finish then dur = finish - start end
    if not dur or dur <= 0 then return nil, "Resolve did not report the clip's length" end

    local clip_fps = num(try(function() return mpi:GetClipProperty("FPS") end))
    local expected = nil
    if clip_fps and clip_fps > 0 and tl_fps and tl_fps > 0 then
        expected = tl_fps / clip_fps
        if math.abs(expected - 1) < 0.0005 then expected = 1 end
    end

    local s0 = num(try(function() return item:GetSourceStartFrame() end))
    local s1 = num(try(function() return item:GetSourceEndFrame() end))
    local left = num(try(function() return item:GetLeftOffset() end))
    local limit = REQUEST.num_frames

    local notes = {}
    local ratio

    -- A source start beyond the end of the clip isn't counted from the clip's first frame (it includes the timecode, say)
    local s0_ok = s0 ~= nil and s0 >= 0 and (limit == nil or s0 <= limit)

    if s0_ok then
        ratio = expected or 1
        if s1 ~= nil and s1 > s0 then
            local span = s1 - s0
            if expected then
                local diff = math.abs(dur - expected * span)
                if diff > math.max(2, 2 * expected) + 0.002 * dur then
                    ratio = dur / span
                    notes[#notes + 1] = "the clip is retimed, so the marker positions are approximate"
                end
            else
                ratio = dur / span
                if math.abs(ratio - 1) < 0.02 then ratio = 1 end
            end
        end
        if left == nil or left < 0 then left = round(s0 * ratio) end
    else
        -- Resolve before 19.0.2, or a source start that can't be used: the left offset, in timeline frames
        if left == nil or left < 0 then
            return nil, "Resolve did not report which part of the clip is on the timeline"
        end
        ratio = expected or 1
        s0 = left
        if ratio ~= 1 then
            s0 = left / ratio
            notes[#notes + 1] = "the frame rates differ and this Resolve version doesn't tell the exact source frame, so the marker positions are approximate"
        end
    end

    return {
        start = start, dur = dur, left = left,
        offset = function(src) return round((src - s0) * ratio) end,
    }, notes
end

local function main()
    local resolve = get_resolve()
    if not resolve then
        log("error: no resolve")
        return
    end
    local pm = try(function() return resolve:GetProjectManager() end)
    local project = pm and try(function() return pm:GetCurrentProject() end)
    local timeline = project and try(function() return project:GetCurrentTimeline() end)
    if not timeline then
        log("no active timeline, nothing done")
        return
    end

    local video = base_name(REQUEST.video)
    local clips, by_name = find_clips(collect_items(timeline))
    if #clips == 0 then
        log("%s is not on the active timeline, nothing done", video)
        return
    end
    if by_name then
        log("%s is in another folder than the clip on the timeline, matched by file name and frame count", video)
    end

    -- What was made here before for this video, on the ruler and on the clips: the markers go to one of them
    local prefix = "gyroflow-trim|" .. video:lower() .. "|"
    local function remove_own_markers(owner)
        local removed = 0
        local markers = try(function() return owner:GetMarkers() end)
        if type(markers) ~= "table" then return 0 end
        for frame, m in pairs(markers) do
            if type(m) == "table" and type(m.customData) == "string" and m.customData:sub(1, #prefix) == prefix then
                if REQUEST.dry_run or try(function() return owner:DeleteMarkerAtFrame(frame) end) then
                    removed = removed + 1
                end
            end
        end
        return removed
    end
    local removed = remove_own_markers(timeline)
    for _, e in ipairs(clips) do
        removed = removed + remove_own_markers(e.item)
    end

    local points = {}
    for i, r in ipairs(REQUEST.ranges) do
        local suffix = (#REQUEST.ranges > 1) and (" " .. int(i)) or ""
        points[#points + 1] = { frame = r[1], name = NAME_IN .. suffix, custom = prefix .. int(i) .. "|in" }
        points[#points + 1] = { frame = r[2], name = NAME_OUT .. suffix, custom = prefix .. int(i) .. "|out" }
    end

    local placed = 0
    local function add_marker(owner, frame_id, p, where)
        local note = video .. ", source frame " .. int(p.frame)
        if REQUEST.dry_run or try(function() return owner:AddMarker(frame_id, REQUEST.color, p.name, note, 1, p.custom) end) then
            placed = placed + 1
            log("\"%s\" %s (source frame %s)", p.name, where, int(p.frame))
        else
            local existing = try(function() return owner:GetMarkers() end)
            existing = type(existing) == "table" and existing[frame_id] or nil
            if type(existing) == "table" then
                log("\"%s\" not added %s, the marker \"%s\" is already there", p.name, where, tostring(existing.name or ""))
            else
                log("Resolve refused \"%s\" %s", p.name, where)
            end
        end
    end

    local tl_fps = num(try(function() return timeline:GetSetting("timelineFrameRate") end))
                or num(try(function() return project:GetSetting("timelineFrameRate") end))
    local mappers = {}
    for _, e in ipairs(clips) do
        local m, notes = make_mapper(e, tl_fps)
        if m then
            for _, n in ipairs(notes) do log("%s", n) end
            m.track = e.track
            m.item = e.item
            mappers[#mappers + 1] = m
        else
            log("skipped a clip on track V%s: %s", int(e.track), notes)
        end
    end

    if REQUEST.target == "timeline" then
        local tl_start = num(try(function() return timeline:GetStartFrame() end)) or 0
        for _, p in ipairs(points) do
            local shown = false
            for _, m in ipairs(mappers) do
                local k = m.offset(p.frame)
                if k >= 0 and k <= m.dur - 1 then
                    shown = true
                    add_marker(timeline, m.start + k - tl_start, p, "at timeline frame " .. int(m.start + k))
                end
            end
            if not shown and #mappers > 0 then
                log("\"%s\" (source frame %s) is outside the part of the clip on the timeline, no marker", p.name, int(p.frame))
            end
        end
    else
        -- A clip's markers are its own (the pieces of a cut clip each have theirs), at frames counted in timeline frames
        -- from the first frame of its video. The ones outside the part of the clip on the timeline show once it's extended
        for _, m in ipairs(mappers) do
            for _, p in ipairs(points) do
                local k = m.offset(p.frame)
                local where = "on the clip on track V" .. int(m.track)
                if k < 0 or k > m.dur - 1 then where = where .. ", outside its part on the timeline" end
                add_marker(m.item, math.max(0, m.left + k), p, where)
            end
        end
    end

    log("done: %s marker(s) placed in %s, %s replaced", int(placed), tostring(REQUEST.color), int(removed))
end

local ok, err = pcall(main)
if not ok then
    log("error: %s", tostring(err))
end
