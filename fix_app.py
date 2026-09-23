f = r'C:\Users\wanganxin\Desktop\项目\cc-switch\src\App.tsx'
with open(f, 'r', encoding='utf-8') as fh:
    content = fh.read()

# 1. Change home page button to trigger openRequestId instead of switching view
old_btn = 'onClick={() => setCurrentView("codexAssistant")}'
new_btn = 'onClick={() => setCodexAssistantOpenRequest((c) => c + 1)}'
if old_btn in content:
    content = content.replace(old_btn, new_btn, 1)
    print('OK: button changed to openRequestId')
else:
    print('NOT FOUND: button')

# 2. Remove the guard so floating dock is always rendered
old_guard = '{currentView !== "codexAssistant" && (\n      <CodexAssistantDock'
new_guard = '{(\n      <CodexAssistantDock'
if old_guard in content:
    content = content.replace(old_guard, new_guard, 1)
    print('OK: removed guard from floating dock')
else:
    print('NOT FOUND: guard')

# 3. Make the codexAssistant case return null (floating dock handles everything)
old_case = '''        case "codexAssistant":
          return (
            <CodexAssistantDock
              embedded
              providerReady={codexProviderReady}
              onOpenCodexConfiguration={() => {
                setActiveApp("codex");
                setCurrentView("providers");
              }}
              onClose={() => setCurrentView("providers")}
            />
          );'''
new_case = '''        case "codexAssistant":
          return null;'''
if old_case in content:
    content = content.replace(old_case, new_case, 1)
    print('OK: codexAssistant case returns null')
else:
    print('NOT FOUND: case')

with open(f, 'w', encoding='utf-8') as fh:
    fh.write(content)
