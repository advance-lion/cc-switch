f = r'C:\Users\wanganxin\Desktop\项目\cc-switch\src\components\assistant\CodexAssistantDock.tsx'
with open(f, 'r', encoding='utf-8') as fh:
    content = fh.read()

old_msg = '          case "message":\n            if (event.message) {\n              setChatMessages((current) => {'
new_msg = '          case "message":\n            if (event.message) {\n              if (!openRef.current) setHasUnread(true);\n              setChatMessages((current) => {'

if old_msg in content:
    content = content.replace(old_msg, new_msg, 1)
    print('OK: message')
else:
    print('NOT FOUND: message')

old_fin = '          case "finished":\n            streamAssistantMessageRef.current = false;\n            setRunning(false);'
new_fin = '          case "finished":\n            if (!openRef.current) setHasUnread(true);\n            streamAssistantMessageRef.current = false;\n            setRunning(false);'

if old_fin in content:
    content = content.replace(old_fin, new_fin, 1)
    print('OK: finished')
else:
    print('NOT FOUND: finished')

with open(f, 'w', encoding='utf-8') as fh:
    fh.write(content)
