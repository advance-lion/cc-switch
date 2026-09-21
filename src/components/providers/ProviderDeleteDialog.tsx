import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { AlertTriangle, Trash2, FileX, Unlink } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { providerCenterApi } from "@/lib/api";
import type { DeleteMode, ProviderCenterApp } from "@/lib/api/providerCenter";
import { refreshProviderCenterApps } from "@/lib/query/providerCenter";
import type { Provider } from "@/types";
import { providerCenterAppLabel } from "./providerCenterApps";
import { extractErrorMessage } from "@/utils/errorUtils";

interface ProviderDeleteDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  provider: Provider | null;
  definitionId: string;
  appType: string;
  boundAgents: string[];
  onSuccess: () => void;
}

export function ProviderDeleteDialog({
  open,
  onOpenChange,
  provider,
  definitionId,
  appType,
  boundAgents,
  onSuccess,
}: ProviderDeleteDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [mode, setMode] = useState<DeleteMode>("removeCurrent");
  const [confirmingGlobal, setConfirmingGlobal] = useState(false);
  const [loading, setLoading] = useState(false);

  const resetState = () => {
    setMode("removeCurrent");
    setConfirmingGlobal(false);
    setLoading(false);
  };

  const handleOpenChange = (next: boolean) => {
    if (!next) resetState();
    onOpenChange(next);
  };

  const handleConfirm = async () => {
    if (!provider) return;
    if (mode === "deleteGlobally" && !confirmingGlobal) {
      setConfirmingGlobal(true);
      return;
    }
    setLoading(true);
    try {
      await providerCenterApi.deleteProvider(definitionId, appType, mode);
      const affectedApps =
        mode === "deleteGlobally"
          ? Array.from(
              new Set(
                [...boundAgents, appType].filter(
                  (app): app is ProviderCenterApp => Boolean(app),
                ),
              ),
            )
          : [appType as ProviderCenterApp];
      await refreshProviderCenterApps(queryClient, affectedApps);
      toast.success(t("provider.deleteSuccess", { defaultValue: "已删除" }), {
        closeButton: true,
      });
      resetState();
      onSuccess();
    } catch (error) {
      toast.error(t("provider.deleteFailed", { defaultValue: "删除失败" }), {
        description: extractErrorMessage(error),
        closeButton: true,
      });
    } finally {
      setLoading(false);
    }
  };

  const options: Array<{
    value: DeleteMode;
    icon: typeof Trash2;
    label: string;
    description: string;
    danger?: boolean;
  }> = [
    {
      value: "removeCurrent",
      icon: FileX,
      label: t("provider.delete.removeCurrent", {
        defaultValue: "从当前 Agent 移除",
      }),
      description: t("provider.delete.removeCurrentDesc", {
        defaultValue: `从 ${providerCenterAppLabel(appType)} 移除投射并解除绑定`,
      }),
    },
    {
      value: "detachKeepIndependent",
      icon: Unlink,
      label: t("provider.delete.detachKeepIndependent", {
        defaultValue: "保留为独立 Provider",
      }),
      description: t("provider.delete.detachKeepIndependentDesc", {
        defaultValue: "保留配置但解除通用 Provider 绑定",
      }),
    },
    {
      value: "deleteGlobally",
      icon: Trash2,
      label: t("provider.delete.deleteGlobally", {
        defaultValue: "从所有 Agent 彻底删除",
      }),
      description: t("provider.delete.deleteGloballyDesc", {
        defaultValue: "删除所有投射、绑定和通用 Provider 定义",
      }),
      danger: true,
    },
  ];

  const showGlobalWarning = mode === "deleteGlobally" && confirmingGlobal;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>
            {t("provider.deleteTitle", { defaultValue: "删除通用 Provider" })}
          </DialogTitle>
          <DialogDescription>
            {provider?.name} —{" "}
            {t("provider.deleteChooseMode", {
              defaultValue: "选择删除方式",
            })}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-2 py-2">
          {options.map((option) => {
            const Icon = option.icon;
            const selected = mode === option.value;
            return (
              <button
                key={option.value}
                type="button"
                disabled={loading}
                onClick={() => {
                  setMode(option.value);
                  setConfirmingGlobal(false);
                }}
                className={`flex w-full items-start gap-3 rounded-lg border p-3 text-left transition-colors ${
                  selected
                    ? option.danger
                      ? "border-destructive bg-destructive/5"
                      : "border-primary bg-primary/5"
                    : "border-border-default hover:bg-muted"
                }`}
              >
                <Icon
                  className={`mt-0.5 h-5 w-5 flex-shrink-0 ${
                    option.danger ? "text-destructive" : "text-muted-foreground"
                  }`}
                />
                <div className="flex-1">
                  <div
                    className={`text-sm font-medium ${
                      option.danger ? "text-destructive" : ""
                    }`}
                  >
                    {option.label}
                  </div>
                  <div className="text-xs text-muted-foreground">
                    {option.description}
                  </div>
                </div>
              </button>
            );
          })}
        </div>

        {showGlobalWarning && (
          <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3">
            <AlertTriangle className="mt-0.5 h-5 w-5 flex-shrink-0 text-destructive" />
            <div className="text-sm">
              <div className="font-medium text-destructive">
                {t("provider.delete.globalWarningTitle", {
                  defaultValue: "确认彻底删除",
                })}
              </div>
              <div className="text-muted-foreground">
                {t("provider.delete.globalWarningDesc", {
                  defaultValue: `将影响以下 Agent: ${boundAgents
                    .map((a) => providerCenterAppLabel(a))
                    .join("、")}`,
                })}
              </div>
            </div>
          </div>
        )}

        <DialogFooter>
          <Button
            variant="outline"
            disabled={loading}
            onClick={() => handleOpenChange(false)}
          >
            {t("common.cancel", { defaultValue: "取消" })}
          </Button>
          <Button
            variant={mode === "deleteGlobally" ? "destructive" : "default"}
            disabled={loading}
            onClick={() => void handleConfirm()}
          >
            {loading
              ? t("common.processing", { defaultValue: "处理中…" })
              : showGlobalWarning
                ? t("provider.delete.confirmGlobal", {
                    defaultValue: "确认彻底删除",
                  })
                : t("common.confirm", { defaultValue: "确认" })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
