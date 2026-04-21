# TlsResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**AcmeAvailable** | **bool** | Whether ACME auto-provisioning is available. | 
**AcmeEmail** | Pointer to **NullableString** | ACME email address, if configured. | [optional] 
**Certificates** | [**[]CertInfo**](CertInfo.md) | Certificate details. | 
**Total** | **int32** | Total number of cached certificates. | 

## Methods

### NewTlsResponse

`func NewTlsResponse(acmeAvailable bool, certificates []CertInfo, total int32, ) *TlsResponse`

NewTlsResponse instantiates a new TlsResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewTlsResponseWithDefaults

`func NewTlsResponseWithDefaults() *TlsResponse`

NewTlsResponseWithDefaults instantiates a new TlsResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAcmeAvailable

`func (o *TlsResponse) GetAcmeAvailable() bool`

GetAcmeAvailable returns the AcmeAvailable field if non-nil, zero value otherwise.

### GetAcmeAvailableOk

`func (o *TlsResponse) GetAcmeAvailableOk() (*bool, bool)`

GetAcmeAvailableOk returns a tuple with the AcmeAvailable field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAcmeAvailable

`func (o *TlsResponse) SetAcmeAvailable(v bool)`

SetAcmeAvailable sets AcmeAvailable field to given value.


### GetAcmeEmail

`func (o *TlsResponse) GetAcmeEmail() string`

GetAcmeEmail returns the AcmeEmail field if non-nil, zero value otherwise.

### GetAcmeEmailOk

`func (o *TlsResponse) GetAcmeEmailOk() (*string, bool)`

GetAcmeEmailOk returns a tuple with the AcmeEmail field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAcmeEmail

`func (o *TlsResponse) SetAcmeEmail(v string)`

SetAcmeEmail sets AcmeEmail field to given value.

### HasAcmeEmail

`func (o *TlsResponse) HasAcmeEmail() bool`

HasAcmeEmail returns a boolean if a field has been set.

### SetAcmeEmailNil

`func (o *TlsResponse) SetAcmeEmailNil(b bool)`

 SetAcmeEmailNil sets the value for AcmeEmail to be an explicit nil

### UnsetAcmeEmail
`func (o *TlsResponse) UnsetAcmeEmail()`

UnsetAcmeEmail ensures that no value is present for AcmeEmail, not even an explicit nil
### GetCertificates

`func (o *TlsResponse) GetCertificates() []CertInfo`

GetCertificates returns the Certificates field if non-nil, zero value otherwise.

### GetCertificatesOk

`func (o *TlsResponse) GetCertificatesOk() (*[]CertInfo, bool)`

GetCertificatesOk returns a tuple with the Certificates field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCertificates

`func (o *TlsResponse) SetCertificates(v []CertInfo)`

SetCertificates sets Certificates field to given value.


### GetTotal

`func (o *TlsResponse) GetTotal() int32`

GetTotal returns the Total field if non-nil, zero value otherwise.

### GetTotalOk

`func (o *TlsResponse) GetTotalOk() (*int32, bool)`

GetTotalOk returns a tuple with the Total field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTotal

`func (o *TlsResponse) SetTotal(v int32)`

SetTotal sets Total field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


