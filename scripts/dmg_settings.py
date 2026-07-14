import os.path

app = defines.get("app", "dist/Hedos.app")
appname = os.path.basename(app)

format = "UDBZ"
files = [app]
symlinks = {"Applications": "/Applications"}
background = defines.get("background", "dist/.dmg-assets/background.png")

window_rect = ((180, 140), (820, 500))
default_view = "icon-view"
show_status_bar = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 128
text_size = 13
icon_locations = {
    appname: (215, 250),
    "Applications": (455, 250),
}
