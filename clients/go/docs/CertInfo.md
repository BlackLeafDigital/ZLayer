# CertInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Domain** | **string** | Domain the certificate covers. | 
**Fingerprint** | Pointer to **NullableString** | SHA-256 fingerprint, if metadata is available. | [optional] 
**NeedsRenewal** | **bool** | Whether the certificate needs renewal (within 30 days of expiry). | 
**NotAfter** | Pointer to **NullableString** | Certificate expiry (ISO-8601), if metadata is available. | [optional] 
**NotBefore** | Pointer to **NullableString** | Certificate validity start (ISO-8601), if metadata is available. | [optional] 

## Methods

### NewCertInfo

`func NewCertInfo(domain string, needsRenewal bool, ) *CertInfo`

NewCertInfo instantiates a new CertInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCertInfoWithDefaults

`func NewCertInfoWithDefaults() *CertInfo`

NewCertInfoWithDefaults instantiates a new CertInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetDomain

`func (o *CertInfo) GetDomain() string`

GetDomain returns the Domain field if non-nil, zero value otherwise.

### GetDomainOk

`func (o *CertInfo) GetDomainOk() (*string, bool)`

GetDomainOk returns a tuple with the Domain field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDomain

`func (o *CertInfo) SetDomain(v string)`

SetDomain sets Domain field to given value.


### GetFingerprint

`func (o *CertInfo) GetFingerprint() string`

GetFingerprint returns the Fingerprint field if non-nil, zero value otherwise.

### GetFingerprintOk

`func (o *CertInfo) GetFingerprintOk() (*string, bool)`

GetFingerprintOk returns a tuple with the Fingerprint field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFingerprint

`func (o *CertInfo) SetFingerprint(v string)`

SetFingerprint sets Fingerprint field to given value.

### HasFingerprint

`func (o *CertInfo) HasFingerprint() bool`

HasFingerprint returns a boolean if a field has been set.

### SetFingerprintNil

`func (o *CertInfo) SetFingerprintNil(b bool)`

 SetFingerprintNil sets the value for Fingerprint to be an explicit nil

### UnsetFingerprint
`func (o *CertInfo) UnsetFingerprint()`

UnsetFingerprint ensures that no value is present for Fingerprint, not even an explicit nil
### GetNeedsRenewal

`func (o *CertInfo) GetNeedsRenewal() bool`

GetNeedsRenewal returns the NeedsRenewal field if non-nil, zero value otherwise.

### GetNeedsRenewalOk

`func (o *CertInfo) GetNeedsRenewalOk() (*bool, bool)`

GetNeedsRenewalOk returns a tuple with the NeedsRenewal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNeedsRenewal

`func (o *CertInfo) SetNeedsRenewal(v bool)`

SetNeedsRenewal sets NeedsRenewal field to given value.


### GetNotAfter

`func (o *CertInfo) GetNotAfter() string`

GetNotAfter returns the NotAfter field if non-nil, zero value otherwise.

### GetNotAfterOk

`func (o *CertInfo) GetNotAfterOk() (*string, bool)`

GetNotAfterOk returns a tuple with the NotAfter field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNotAfter

`func (o *CertInfo) SetNotAfter(v string)`

SetNotAfter sets NotAfter field to given value.

### HasNotAfter

`func (o *CertInfo) HasNotAfter() bool`

HasNotAfter returns a boolean if a field has been set.

### SetNotAfterNil

`func (o *CertInfo) SetNotAfterNil(b bool)`

 SetNotAfterNil sets the value for NotAfter to be an explicit nil

### UnsetNotAfter
`func (o *CertInfo) UnsetNotAfter()`

UnsetNotAfter ensures that no value is present for NotAfter, not even an explicit nil
### GetNotBefore

`func (o *CertInfo) GetNotBefore() string`

GetNotBefore returns the NotBefore field if non-nil, zero value otherwise.

### GetNotBeforeOk

`func (o *CertInfo) GetNotBeforeOk() (*string, bool)`

GetNotBeforeOk returns a tuple with the NotBefore field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNotBefore

`func (o *CertInfo) SetNotBefore(v string)`

SetNotBefore sets NotBefore field to given value.

### HasNotBefore

`func (o *CertInfo) HasNotBefore() bool`

HasNotBefore returns a boolean if a field has been set.

### SetNotBeforeNil

`func (o *CertInfo) SetNotBeforeNil(b bool)`

 SetNotBeforeNil sets the value for NotBefore to be an explicit nil

### UnsetNotBefore
`func (o *CertInfo) UnsetNotBefore()`

UnsetNotBefore ensures that no value is present for NotBefore, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


