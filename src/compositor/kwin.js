// Loaded into KWin by `we-linux daemon`: tells the daemon, over D-Bus, what is on each
// screen whenever that may have changed. KWin has no other way to say which windows
// cover which screen; the daemon decides what to do with it.
const daemon = "@DAEMON@";
const path = "/io/github/MoonTiwzy/WeLinux";
const iface = "io.github.MoonTiwzy.WeLinux.Desktop";

// Screens whose power is off, by name: KWin only says when that changes.
const asleep = {};
// Screens whose signals are followed already, by name.
const watched = {};

function desktopOf(screen) {
    const desktop = workspace.currentDesktopForScreen
        ? workspace.currentDesktopForScreen(screen)
        : workspace.currentDesktop;
    return desktop ? desktop.id : "";
}

function report() {
    const screens = workspace.screens.map(screen => ({
        name: screen.name,
        desktop: desktopOf(screen),
        asleep: asleep[screen.name] === true,
    }));
    const windows = workspace.stackingOrder
        .filter(w => w.managed && !w.deleted && (w.normalWindow || w.dialog || w.utility))
        .map(w => ({
            output: w.output ? w.output.name : "",
            desktops: w.onAllDesktops ? [] : w.desktops.map(d => d.id),
            activities: w.activities,
            fullscreen: w.fullScreen,
            maximized: w.maximizeMode === 3,
            minimized: w.minimized,
            hidden: w.hidden === true,
            active: w.active,
        }));
    callDBus(daemon, path, iface, "Report", JSON.stringify({
        activity: workspace.currentActivity,
        screens: screens,
        windows: windows,
    }));
}

function follow(window) {
    for (const signal of ["fullScreenChanged", "maximizedChanged", "minimizedChanged",
                          "outputChanged", "desktopsChanged", "activitiesChanged", "hiddenChanged"]) {
        if (window[signal]) {
            window[signal].connect(report);
        }
    }
}

function watchScreen(screen) {
    if (watched[screen.name]) {
        return;
    }
    watched[screen.name] = true;
    if (screen.aboutToTurnOff) {
        screen.aboutToTurnOff.connect(() => { asleep[screen.name] = true; report(); });
    }
    if (screen.wakeUp) {
        screen.wakeUp.connect(() => { asleep[screen.name] = false; report(); });
    }
}

workspace.stackingOrder.forEach(follow);
workspace.screens.forEach(watchScreen);
workspace.windowAdded.connect(window => { follow(window); report(); });
workspace.windowRemoved.connect(report);
workspace.windowActivated.connect(report);
workspace.currentDesktopChanged.connect(report);
workspace.currentActivityChanged.connect(report);
workspace.screensChanged.connect(() => { workspace.screens.forEach(watchScreen); report(); });
report();
