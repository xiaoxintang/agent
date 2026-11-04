import { Card, Modal } from "antd";
import type { ModalProps } from "antd";

export default function PanelContainer({
  page,
  ...props
}: ModalProps & { page?: boolean }) {
  return page ? (
    <Card title={props.title}>{props.children}</Card>
  ) : (
    <Modal {...props} />
  );
}
