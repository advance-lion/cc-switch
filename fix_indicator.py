f = r'C:\Users\wanganxin\Desktop\项目\cc-switch\src\components\assistant\CodexAssistantDock.tsx'
with open(f, 'r', encoding='utf-8') as fh:
    content = fh.read()

old = """          <CodexIcon size={19} className="dark:invert-0" />
        </button>"""

new = """          <CodexIcon size={19} className="dark:invert-0" />
          {(running || hasUnread) && (
            <span className="pointer-events-none absolute -right-0.5 -top-0.5 flex h-3 w-3">
              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-violet-400 opacity-75" />
              <span className="relative inline-flex h-3 w-3 rounded-full bg-violet-500" />
            </span>
          )}
        </button>"""

if old in content:
    content = content.replace(old, new, 1)
    with open(f, 'w', encoding='utf-8') as fh:
        fh.write(content)
    print('OK: indicator added')
else:
    print('NOT FOUND')
