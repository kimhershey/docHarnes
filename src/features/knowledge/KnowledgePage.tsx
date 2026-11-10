import { Navigate } from "react-router";

import { COMMON_REFERENCE_ID, SCREEN_ROUTES } from "@/lib/types";

/**
 * 0.5.0: the 지식 베이스 screen is gone — its groups and documents live on the projects screen (공통 참고 자료 and
 * each project's 참고 자료 tab) and the advanced panels under 설정 > 참고 자료. The old route forwards there.
 */
export function KnowledgePage() {
  return <Navigate to={`${SCREEN_ROUTES.projects}/${COMMON_REFERENCE_ID}`} replace />;
}
