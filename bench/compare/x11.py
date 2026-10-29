"""The few Xlib calls the comparison needs, through ctypes, so it installs nothing.

Everything here talks to a *private* X server (a rootful Xwayland the harness starts), so
synthetic keys and clicks never reach the desktop the benchmark is run from.
"""

import ctypes
import ctypes.util

_x = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
_xtst = ctypes.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so.6")

ZPIXMAP = 2
ALL_PLANES = ctypes.c_ulong(~0 & 0xFFFFFFFFFFFFFFFF)
IS_VIEWABLE = 2


class XImage(ctypes.Structure):
    # Only the leading fields; the struct is never allocated on this side.
    _fields_ = [
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("xoffset", ctypes.c_int),
        ("format", ctypes.c_int),
        ("data", ctypes.c_void_p),
        ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int),
        ("bitmap_bit_order", ctypes.c_int),
        ("bitmap_pad", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("bytes_per_line", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int),
    ]


class XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int),
        ("y", ctypes.c_int),
        ("width", ctypes.c_int),
        ("height", ctypes.c_int),
        ("border_width", ctypes.c_int),
        ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p),
        ("root", ctypes.c_ulong),
        ("class_", ctypes.c_int),
        ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int),
        ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong),
        ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int),
        ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int),
        ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long),
        ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int),
        ("screen", ctypes.c_void_p),
    ]


_x.XOpenDisplay.restype = ctypes.c_void_p
_x.XOpenDisplay.argtypes = [ctypes.c_char_p]
_x.XDefaultRootWindow.restype = ctypes.c_ulong
_x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
_x.XGetImage.restype = ctypes.POINTER(XImage)
_x.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                         ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int]
_x.XDestroyImage.argtypes = [ctypes.POINTER(XImage)]
_x.XQueryTree.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong),
                          ctypes.POINTER(ctypes.c_ulong),
                          ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
                          ctypes.POINTER(ctypes.c_uint)]
_x.XGetWindowAttributes.argtypes = [ctypes.c_void_p, ctypes.c_ulong,
                                    ctypes.POINTER(XWindowAttributes)]
_x.XMoveResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                 ctypes.c_uint, ctypes.c_uint]
_x.XFree.argtypes = [ctypes.c_void_p]
_x.XFlush.argtypes = [ctypes.c_void_p]
_x.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
_x.XStringToKeysym.restype = ctypes.c_ulong
_x.XStringToKeysym.argtypes = [ctypes.c_char_p]
_x.XKeysymToKeycode.restype = ctypes.c_ubyte
_x.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
_x.XSetInputFocus.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
_xtst.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
_xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int,
                                       ctypes.c_ulong]
_xtst.XTestFakeMotionEvent.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                                       ctypes.c_ulong]


_ERROR_HANDLER = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
# Electron creates and destroys helper windows during startup, so a window listed by XQueryTree
# can be gone by the time its attributes are asked for. Xlib's default answer is to exit.
_ignore_errors = _ERROR_HANDLER(lambda _dpy, _event: 0)
_x.XSetErrorHandler.argtypes = [_ERROR_HANDLER]
_x.XSetErrorHandler(_ignore_errors)


class Display:
    def __init__(self, name: str):
        self.dpy = _x.XOpenDisplay(name.encode())
        if not self.dpy:
            raise RuntimeError(f"cannot open X display {name}")
        self.root = _x.XDefaultRootWindow(self.dpy)

    def grab(self, x=0, y=0, w=1600, h=1000) -> bytes:
        """The pixels of a rectangle of the root window, as raw bytes."""
        img = _x.XGetImage(self.dpy, self.root, x, y, w, h, ALL_PLANES, ZPIXMAP)
        if not img:
            raise RuntimeError("XGetImage failed")
        data = ctypes.string_at(img.contents.data, img.contents.bytes_per_line * h)
        _x.XDestroyImage(img)
        return data

    def viewable_windows(self):
        """Top-level windows that are mapped, with their geometry."""
        root_ret, parent_ret = ctypes.c_ulong(), ctypes.c_ulong()
        children = ctypes.POINTER(ctypes.c_ulong)()
        n = ctypes.c_uint()
        _x.XQueryTree(self.dpy, self.root, ctypes.byref(root_ret), ctypes.byref(parent_ret),
                      ctypes.byref(children), ctypes.byref(n))
        found = []
        for i in range(n.value):
            attrs = XWindowAttributes()
            _x.XGetWindowAttributes(self.dpy, children[i], ctypes.byref(attrs))
            if attrs.map_state == IS_VIEWABLE and attrs.width > 200 and attrs.height > 200:
                found.append((children[i], attrs.width, attrs.height))
        if children:
            _x.XFree(children)
        return found

    def place(self, window: int, w: int, h: int):
        # There is no window manager on the private server, so nothing else will size the window.
        _x.XMoveResizeWindow(self.dpy, window, 0, 0, w, h)
        _x.XSync(self.dpy, 0)

    def focus(self, window: int):
        _x.XSetInputFocus(self.dpy, window, 1, 0)
        _x.XSync(self.dpy, 0)

    def click(self, x: int, y: int):
        _xtst.XTestFakeMotionEvent(self.dpy, 0, x, y, 0)
        _xtst.XTestFakeButtonEvent(self.dpy, 1, 1, 0)
        _xtst.XTestFakeButtonEvent(self.dpy, 1, 0, 0)
        _x.XSync(self.dpy, 0)

    def keycode(self, keysym_name: str) -> int:
        return _x.XKeysymToKeycode(self.dpy, _x.XStringToKeysym(keysym_name.encode()))

    def key(self, code: int, press: bool):
        _xtst.XTestFakeKeyEvent(self.dpy, code, 1 if press else 0, 0)
        _x.XFlush(self.dpy)
