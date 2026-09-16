import { Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type {
  ProviderApplyPreviewTarget,
  ProviderCenterApp,
} from "@/lib/api/providerCenter";
import {
  PROVIDER_CENTER_APPS,
  providerCenterAppLabel,
} from "@/components/providers/providerCenterApps";

interface UniversalProviderTargetDialogProps {
  open: boolean;
  targets: ProviderApplyPreviewTarget[];
  selectedAppTypes: ProviderCenterApp[];
  pending: boolean;
  onSelectedAppTypesChange: (appTypes: ProviderCenterApp[]) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

export function UniversalProviderTargetDialog({
  open,
  targets,
  selectedAppTypes,
  pending,
  onSelectedAppTypesChange,
  onConfirm,
  onCancel,
}: UniversalProviderTargetDialogProps) {
  const selected = new Set(selectedAppTypes);
  const targetByApp = new Map(targets.map((target) => [target.appType, target]));

  const toggle = (appType: ProviderCenterApp, checked: boolean) => {
    const next = new Set(selected);
    if (checked) next.add(appType);
    else next.delete(appType);
    onSelectedAppTypesChange(
      PROVIDER_CENTER_APPS.filter((candidate) => next.has(candidate)),
    );
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onCancel()}>
      <DialogContent className="max-w-lg" zIndex="top">
        <DialogHeader>
          <DialogTitle>选择使用此 Provider 的 Agent</DialogTitle>
          <DialogDescription>
            已默认选择所有兼容的 Agent。需要路由的 Agent 也会保存 Provider；实际使用时需开启本地路由。保存不会切换当前 Provider，也不会自动启动路由。
          </DialogDescription>
        </DialogHeader>

        <div className="max-h-[55vh] space-y-2 overflow-y-auto py-1">
          {PROVIDER_CENTER_APPS.map((appType) => {
            const target = targetByApp.get(appType);
            const compatible = target?.compatible ?? false;
            const mode = target?.connectionMode ??
              (compatible ? "direct" : "unsupported");
            const disabled = pending || !compatible;

            return (
              <label
                key={appType}
                className={`flex items-start gap-3 rounded-lg border p-3 ${
                  disabled
                    ? "cursor-not-allowed border-border-default opacity-60"
                    : "cursor-pointer border-border-default hover:bg-accent/50"
                }`}
              >
                <Checkbox
                  aria-label={providerCenterAppLabel(appType)}
                  checked={selected.has(appType)}
                  disabled={disabled}
                  onCheckedChange={(checked) => toggle(appType, checked)}
                  className="mt-0.5"
                />
                <span className="min-w-0 flex-1">
                  <span className="flex flex-wrap items-center gap-2">
                    <span className="text-sm font-medium">
                      {providerCenterAppLabel(appType)}
                    </span>
                    {mode === "direct" && (
                      <span className="rounded bg-emerald-100 px-1.5 py-0.5 text-[10px] font-semibold text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-200">
                        直接兼容
                      </span>
                    )}
                    {mode === "proxy" && (
                      <span className="rounded bg-amber-100 px-1.5 py-0.5 text-[10px] font-semibold text-amber-700 dark:bg-amber-900/40 dark:text-amber-200">
                        需要路由
                      </span>
                    )}
                    {mode === "unsupported" && (
                      <span className="rounded bg-red-100 px-1.5 py-0.5 text-[10px] font-semibold text-red-700 dark:bg-red-900/40 dark:text-red-200">
                        不兼容
                      </span>
                    )}
                  </span>
                  {target?.message && (
                    <span className="mt-1 block text-xs text-muted-foreground">
                      {target.message}
                    </span>
                  )}
                </span>
              </label>
            );
          })}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onCancel} disabled={pending}>
            取消
          </Button>
          <Button
            onClick={onConfirm}
            disabled={pending || selectedAppTypes.length === 0}
          >
            {pending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            预览并继续
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
