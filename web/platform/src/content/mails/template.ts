export interface EmailData {
  email: string;
  time: string;
}

export const subscription = (data: EmailData): string => {
  return `
    <p>New subscription from <strong>${data.email}</strong></p>
    <p>Time: ${data.time}</p>
  `;
};

export const thanks = (): string => {
  return `
    <p>Hello,</p>
    <p>Thank you for your subscription.</p>
    <p>We will get back to you as soon as possible!</p>
    <p>Best regards,<br>The Nativelink Team</p>
  `;
};
