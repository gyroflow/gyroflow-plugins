-- Shows the marker colors as a row of colored dots, like the marker dialog of Resolve, and sets the one that is clicked
-- as the trim marker color of the Gyroflow node it was opened from. Run through fuscript by the plugin (see
-- trim_markers.rs), which defines REQUEST above this text:
--   colors    { { name, "#rrggbb" }, ... } in the order of the color parameter
--   current   the index of the node's color, from 0
--   instance  the node's instance id
--   cursor    { x, y } where the mouse is, in the coordinates of the user interface, nil when unknown
--   screen    { left, top, right, bottom } of the screen the mouse is on, nil when unknown
--   id        a number of its own for each palette opened
--   seconds   close it by itself after that long, a minute when not given

-- Of a dot, between two of them, and around them
local SIZE, GAP, MARGIN = 20, 7, 10
local ESCAPE = 16777216 -- Qt::Key_Escape
-- Of its own: the windows of the scripts that ended are still there, hidden
local WINDOW = "GyroflowTrimMarkerColor" .. tostring(REQUEST.id or "") .. "_" .. tostring(os.time())

local function try(f)
    local ok, r = pcall(f)
    if ok then return r end
    return nil
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

local function main()
    local resolve = get_resolve()
    local fusion = resolve and try(function() return resolve:Fusion() end)
    local ui = fusion and fusion.UIManager
    if not ui then
        print("error: no resolve")
        return
    end

    local dots = { Spacing = GAP, Weight = 0 }
    for i, color in ipairs(REQUEST.colors) do
        local ring = (i - 1 == REQUEST.current) and "#FFFFFF" or color[2]
        dots[#dots + 1] = ui:Button{
            ID = "Color" .. (i - 1), Text = "", ToolTip = color[1], Flat = true,
            MinimumSize = { SIZE, SIZE }, MaximumSize = { SIZE, SIZE },
            StyleSheet = string.format("QPushButton { background-color: %s; border: 2px solid %s; border-radius: %dpx; } QPushButton:hover { border: 2px solid #FFFFFF; }",
                color[2], ring, SIZE / 2),
        }
    end
    local width = #REQUEST.colors * SIZE + (#REQUEST.colors - 1) * GAP + 2 * MARGIN
    local height = SIZE + 2 * MARGIN

    local window = {
        ID = WINDOW, WindowTitle = "Trim marker color",
        StyleSheet = "QWidget#" .. WINDOW .. " { background-color: #28282E; border: 1px solid #5A5A62; }",
        -- The window doesn't close by itself: it tells of what would close it
        Events = { Close = true, MousePress = true, KeyPress = true, KeyRelease = true },
        ui:HGroup(dots),
    }
    if REQUEST.cursor then
        -- Under the mouse, the node's color at it, and all of it on the screen. A popup gets the clicks made elsewhere
        local x = REQUEST.cursor[1] - MARGIN - (REQUEST.current or 0) * (SIZE + GAP) - SIZE / 2
        local y = REQUEST.cursor[2] + 14
        if REQUEST.screen then
            x = math.max(REQUEST.screen[1], math.min(x, REQUEST.screen[3] - width))
            if y + height > REQUEST.screen[4] then y = REQUEST.cursor[2] - 14 - height end
            y = math.max(REQUEST.screen[2], y)
        end
        window.Geometry = { math.floor(x), math.floor(y), width, height }
        window.WindowFlags = { Popup = true }
    else
        window.FixedSize = { width, height }
        window.WindowFlags = { Dialog = true, WindowStaysOnTopHint = true }
    end
    local win = ui:Window(window)
    local timer = nil

    -- The events are taken one by one rather than through bmd.UIDispatcher, which fails on those of a window it doesn't
    -- know: what comes for the windows of the scripts that ended is handed to the ones still running
    local chosen = nil
    local ok, err = pcall(function()
        timer = ui:Timer{ ID = WINDOW .. "Timer", Interval = (REQUEST.seconds or 60) * 1000, SingleShot = true }
        win:Show()
        timer:Start()
        local done = false
        while not done do
            local ev = ui:GetEvent(true)
            while ev and not done do
                if ev.window == WINDOW then
                    local dot = tostring(ev.who):match("^Color(%d+)$")
                    if ev.what == "Clicked" and dot then
                        chosen = tonumber(dot)
                        done = true
                    elseif ev.what == "Close" then
                        done = true
                    elseif ev.what == "MousePress" then
                        -- A click elsewhere closes it
                        local at = ev.Pos
                        done = not at or at[1] < 0 or at[2] < 0 or at[1] >= width or at[2] >= height
                    elseif ev.what == "KeyPress" or ev.what == "KeyRelease" then
                        done = ev.Key == ESCAPE
                    end
                elseif ev.what == "Timeout" and ev.who == WINDOW .. "Timer" then
                    -- It doesn't stay forever
                    done = true
                end
                if not done then ev = ui:GetEvent(false) end
            end
        end
    end)
    try(function() timer:Stop() end)
    try(function() win:Hide() end)
    -- What still comes for the window isn't left for another script to get. Going to another application asks it to
    -- close a thousand times over
    local quiet = 0
    for _ = 1, 3000 do
        if try(function() return ui:GetEvent(false) end) then
            quiet = 0
        else
            quiet = quiet + 1
            if quiet >= 3 then break end
            try(function() bmd.wait(0.03) end)
        end
    end
    if not ok then error(err, 0) end

    if not chosen or not REQUEST.colors[chosen + 1] then
        print("no color picked")
        return
    end
    -- The node it was opened from, and the copies of it with it: in the comp that is open, or else in the comps of the
    -- clip under the playhead
    local set = 0
    local function set_in(comp)
        if not comp then return end
        for _, tool in pairs(try(function() return comp:GetToolList(false) end) or {}) do
            if tostring(tool:GetAttrs().TOOLS_RegID) == "ofx.xyz.gyroflow" and tostring(tool:GetInput("InstanceId")) == REQUEST.instance then
                tool:SetInput("TrimMarkerColor", chosen)
                set = set + 1
            end
        end
    end
    set_in(try(function() return fusion:GetCurrentComp() end))
    if set == 0 then
        local item = try(function() return resolve:GetProjectManager():GetCurrentProject():GetCurrentTimeline():GetCurrentVideoItem() end)
        for i = 1, item and try(function() return item:GetFusionCompCount() end) or 0 do
            set_in(try(function() return item:GetFusionCompByIndex(i) end))
        end
    end
    print(string.format("%s picked, set on %d node(s)", tostring(REQUEST.colors[chosen + 1][1]), set))
end

local ok, err = pcall(main)
if not ok then
    print("error: " .. tostring(err))
end
