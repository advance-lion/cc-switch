import { useCallback, useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Loader2, Plus, X } from "lucide-react";
import { toast } from "sonner";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { providerCenterApi } from "@/lib/api";
import type { ProviderCenterApp } from "@/lib/api/providerCenter";
import type {
  ProviderApplyPreviewTarget,
  ProviderBinding,
} from "@/lib/api/providerCenter";
import {
  PROVIDER_CENTER_APPS,
  providerCenterAppLabel,
} from "./providerCenterApps";
import { extractErrorMessage } from "@/utils/errorUtils";
import { refreshProviderCenterApps } from "@/lib/query/providerCenter";

interface ProviderScopeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  definitionId: string;
  definitionName: string;
}

export function ProviderScopeDialog({
  open,
  onOpenChange,
  definitionId,
  definitionName,
}: ProviderScopeDialogProps) {
  const queryClient = useQueryClient();
  const [bindings, setBindings] = useState<ProviderBinding[]>([]);
  const [targets, setTargets] = useState<ProviderApplyPreviewTarget[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyApp, setBusyApp] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!definitionId) return;
    setLoading(true);
    try {
      const [state, preview] = await Promise.all([
        providerCenterApi.get(),
        providerCenterApi.previewDefinitionCompatibility(
          definitionId,
          PROVIDER_CENTER_APPS,
        ),
      ]);
      setBindings(state.bindings.filter((b) => b.providerId === definitionId));
      setTargets(preview.targets);
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setLoading(false);
    }
  }, [definitionId]);

  useEffect(() => {
    if (open) {
      void refresh();
    }
  }, [open, refresh]);

  const handleAttach = useCallback(
    async (appType: ProviderCenterApp) => {
      setBusyApp(appType);
      let attached = false;
      try {
        await providerCenterApi.attach(definitionId, appType);
        attached = true;
        const preview = await providerCenterApi.previewApply(definitionId, [
          appType,
        ]);
        const transaction = await providerCenterApi.applyTransaction(
          definitionId,
          [appType],
          preview.token,
        );
        if (transaction.status !== "applied") {
          throw new Error(
            `共享配置应用未完成，事务状态：${transaction.status}`,
          );
        }
        await refreshProviderCenterApps(queryClient, [appType]);
        await refresh();
        toast.success(`已添加到 ${providerCenterAppLabel(appType)}`);
      } catch (error) {
        if (attached) {
          await providerCenterApi
            .disableBinding(definitionId, appType, false)
            .catch(() => undefined);
          await refreshProviderCenterApps(queryClient, [appType]).catch(
            () => undefined,
          );
          await refresh().catch(() => undefined);
        }
        toast.error(extractErrorMessage(error));
      } finally {
        setBusyApp(null);
      }
    },
    [definitionId, refresh, queryClient],
  );

  const handleDetach = useCallback(
    async (appType: ProviderCenterApp) => {
      setBusyApp(appType);
      try {
        await providerCenterApi.disableBinding(definitionId, appType, false);
        await refreshProviderCenterApps(queryClient, [appType]);
        await refresh();
        toast.success(`已从 ${providerCenterAppLabel(appType)} 移除`);
      } catch (error) {
        toast.error(extractErrorMessage(error));
      } finally {
        setBusyApp(null);
      }
    },
    [definitionId, refresh, queryClient],
  );

  const bindingFor = (appType: string) =>
    bindings.find((b) => b.appType === appType);

  const targetFor = (appType: string) =>
    targets.find((t) => t.appType === appType);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" zIndex="nested">
        <DialogHeader>
          <DialogTitle className="text-base">
            管理使用范围
            <span className="ml-2 text-sm font-normal text-muted-foreground">
              {definitionName}
            </span>
          </DialogTitle>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center justify-center py-8">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : (
          <div className="space-y-1.5 max-h-[60vh] overflow-y-auto">
            {PROVIDER_CENTER_APPS.map((appType) => {
              const binding = bindingFor(appType);
              const target = targetFor(appType);
              const isBound = binding?.enabled ?? false;
              const isCompatible = target?.compatible ?? false;
              const connectionMode = target?.connectionMode;
              const isBusy = busyApp === appType;

              return (
                <div
                  key={appType}
                  className="flex items-center justify-between rounded-md px-3 py-2 hover:bg-accent/50"
                >
                  <div className="flex items-center gap-2 min-w-0">
                    <span className="text-sm font-medium truncate">
                      {providerCenterAppLabel(appType)}
                    </span>
                    {isBound ? (
                      <span className="shrink-0 rounded bg-emerald-100 px-1.5 py-0.5 text-[10px] font-semibold text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-200">
                        已绑定
                      </span>
                    ) : (
                      <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
                        未绑定
                      </span>
                    )}
                    {connectionMode === "direct" && (
                      <span className="shrink-0 rounded bg-blue-100 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-900/40 dark:text-blue-200">
                        直接兼容
                      </span>
                    )}
                    {connectionMode === "proxy" && (
                      <span className="shrink-0 rounded bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 dark:bg-amber-900/40 dark:text-amber-200">
                        需要路由
                      </span>
                    )}
                    {!isCompatible && target && (
                      <span className="shrink-0 rounded bg-red-100 px-1.5 py-0.5 text-[10px] font-medium text-red-700 dark:bg-red-900/40 dark:text-red-200">
                        不兼容
                      </span>
                    )}
                    {target?.drifted && (
                      <span className="shrink-0 rounded bg-amber-100 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 dark:bg-amber-900/40 dark:text-amber-200">
                        漂移
                      </span>
                    )}
                  </div>

                  {isBusy ? (
                    <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
                  ) : isBound ? (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-7 text-xs text-muted-foreground hover:text-destructive"
                      onClick={() => void handleDetach(appType)}
                      disabled={!isCompatible && !isBound}
                    >
                      <X className="mr-1 h-3 w-3" />
                      移除
                    </Button>
                  ) : (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-7 text-xs"
                      onClick={() => void handleAttach(appType)}
                      disabled={!isCompatible}
                    >
                      <Plus className="mr-1 h-3 w-3" />
                      添加
                    </Button>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
