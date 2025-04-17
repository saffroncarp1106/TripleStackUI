import os
import re
import shutil
from tkinter import Tk, Label, Button, Checkbutton, Frame, IntVar, StringVar, filedialog, messagebox
from tkinter import ttk
from PIL import Image

def update_skin_ini(skin_dir, overlap_value="160"):
    """
    Detects BOM/encoding of skin.ini, removes existing HitCircleOverlap lines,
    reinserts “HitCircleOverlap: <value>” after HitCirclePrefix in [Fonts],
    and writes back preserving original encoding/BOM.
    """
    ini_path = os.path.join(skin_dir, "skin.ini")
    raw = b""
    if os.path.exists(ini_path):
        with open(ini_path, 'rb') as f:
            raw = f.read()
    bom = b""
    if raw.startswith(b'\xff\xfe'):
        enc = 'utf-16-le'; bom = raw[:2]; raw = raw[2:]
    elif raw.startswith(b'\xfe\xff'):
        enc = 'utf-16-be'; bom = raw[:2]; raw = raw[2:]
    else:
        enc = 'latin-1'
    content = raw.decode(enc, errors='ignore')
    content = re.sub(r'(?m)^\s*HitCircleOverlap\s*:\s*.*\r?\n?', '', content)
    lines = content.splitlines(keepends=True)

    new_lines, in_fonts, inserted, fonts_seen = [], False, False, False
    for line in lines:
        sec = re.match(r'^\s*\[([^\]]+)\]\s*$', line)
        if sec:
            if in_fonts and not inserted:
                new_lines.append(f"HitCircleOverlap: {overlap_value}\n")
                inserted = True
            in_fonts = (sec.group(1).lower() == 'fonts')
            if in_fonts: fonts_seen = True
            new_lines.append(line)
            continue
        if line.lstrip().lower().startswith('hitcircleoverlap:'): continue
        if in_fonts and not inserted and line.lstrip().lower().startswith('hitcircleprefix:'):
            new_lines.append(line)
            indent = re.match(r'^(\s*)', line).group(1)
            new_lines.append(f"{indent}\tHitCircleOverlap: {overlap_value}\n")
            inserted = True
            continue
        new_lines.append(line)

    if not fonts_seen:
        new_lines.append('\n[Fonts]\n')
        new_lines.append(f"\tHitCircleOverlap: {overlap_value}\n")
    elif not inserted:
        out, in_f = [], False
        for L in new_lines:
            sec = re.match(r'^\s*\[([^\]]+)\]\s*$', L)
            if sec: in_f = (sec.group(1).lower() == 'fonts')
            if in_f and re.match(r'^\s*\[', L) and not re.match(r'^\s*\[Fonts\]\s*$', L):
                out.append(f"HitCircleOverlap: {overlap_value}\n")
                in_f = False
            out.append(L)
        new_lines = out

    updated = bom + ''.join(new_lines).encode(enc, errors='ignore')
    with open(ini_path, 'wb') as f: f.write(updated)


def delete_2x_assets(skin_dir):
    deleted = []
    for fn in os.listdir(skin_dir):
        if "@2x" in fn.lower():
            p = os.path.join(skin_dir, fn)
            if os.path.isfile(p):
                try: os.remove(p); deleted.append(fn)
                except: pass
    if deleted:
        messagebox.showinfo("Deleted 2x assets", "\n".join(deleted))


def get_unique_folder_name(base_folder):
    unique, i = base_folder, 1
    while os.path.exists(unique): unique = f"{base_folder}({i})"; i+=1
    return unique


def triple_stack_skin_preserve_default_and_hitcircle(
    skin_dir, overlay_filename="hitcircleoverlay.png",
    default_prefix="default-", default_suffix=".png",
    hitcircle_filename="hitcircle.png",
    desired_default_size=(160,160), desired_overlay_size=(160,160)
):
    p = os.path.join(skin_dir, overlay_filename)
    if not os.path.exists(p): messagebox.showerror("Error", f"Missing {overlay_filename}"); return
    ov = Image.open(p).convert("RGBA")
    if ov.size!=desired_overlay_size: ov=ov.resize(desired_overlay_size,Image.NEAREST)
    cnt=0
    for n in range(11):
        fn=f"{default_prefix}{n}{default_suffix}"; fp=os.path.join(skin_dir,fn)
        if not os.path.exists(fp): continue
        cnt+=1; b=Image.open(fp).convert("RGBA"); c=Image.new("RGBA",desired_default_size,(0,0,0,0))
        ox=(desired_default_size[0]-b.width)//2; oy=(desired_default_size[1]-b.height)//2
        c.paste(b,(ox,oy)); c.paste(ov,(0,0),ov); c.save(fp)
    if cnt==0: messagebox.showwarning("Warning","No default-#.png found.")
    hp=os.path.join(skin_dir, hitcircle_filename)
    if os.path.exists(hp):
        h=Image.open(hp).convert("RGBA")
        if force_res.get()!='Native': r=int(force_res.get()); h=h.resize((r,r),Image.NEAREST); ov2=ov.resize((r,r),Image.NEAREST)
        else: ov2=ov.resize(h.size,Image.NEAREST)
        cp=h.copy(); cp.paste(ov2,(0,0),ov2); cp.save(hp)


def force_overlay_file(skin_dir, overlay_filename="hitcircleoverlay.png"):
    p = os.path.join(skin_dir, overlay_filename)
    if os.path.exists(p):
        try:
            im=Image.open(p).convert("RGBA")
            r=int(force_res.get()) if force_res.get()!='Native' else 160
            if im.size!=(r,r): im=im.resize((r,r),Image.NEAREST); im.save(p)
        except: pass


def process_skin():
    d = filedialog.askdirectory(title="Select osu! Skin Folder")
    if not d: return
    if copy_option.get():
        pr, nm=os.path.split(d.rstrip(os.sep)); nn=get_unique_folder_name(os.path.join(pr,nm+"@3xStack"))
        try: shutil.copytree(d,nn); d=nn; messagebox.showinfo("Copy created",nn)
        except Exception as e: messagebox.showerror("Copy error",str(e)); return
    delete_2x_assets(d); triple_stack_skin_preserve_default_and_hitcircle(d)
    update_skin_ini(d)
    if force_overlay.get(): force_overlay_file(d)
    messagebox.showinfo("Complete","Done!")


def main_window():
    root=Tk(); root.option_add("*Font","Arial 10"); root.title("Triple Stack Skin Maker"); root.geometry("600x450"); root.configure(bg="#fff")
    Label(root,text="Shitty Triple Stack Skin Maker",font=("Arial",16,"bold"),bg="#fff").pack(pady=10)
    desc= (
        "This tool modifies:\n"
        "• hitcircle\n"
        "• hitcircleoverlay\n"
        "• default-# images\n\n"
        "Deletes @2x assets, updates skin.ini.\n"
        "BACKUP first!"
    )
    Label(root,text=desc,justify="center",bg="#fff").pack(pady=5)
    ctl=Frame(root,bg="#fff"); ctl.pack(pady=10)
    global copy_option, force_res, force_overlay
    copy_option = IntVar(value=1)
    force_res = StringVar(value="Native")
    force_overlay = IntVar(value=0)
    Checkbutton(ctl,text="Create Copy and Process",variable=copy_option,bg="#fff").pack(pady=2)
    Label(ctl,text="Force resolution, leave off, unless circles are randomly small or large:",bg="#fff").pack()
    combo=ttk.Combobox(ctl,textvariable=force_res,values=["Native","140","150","160"],state="readonly"); combo.pack()
    combo.configure(font=("Arial",10))
    Checkbutton(ctl,text="Enable Force overlay resolution",variable=force_overlay,bg="#fff").pack(pady=2)
    Button(root,text="Select Skin Folder and Process",command=process_skin,width=30,height=2).pack(pady=10)
    Label(root,text="Created by: You\'re Mom",font=("Arial",10),bg="#fff").pack(pady=5)
    root.mainloop()

if __name__=="__main__": main_window()
